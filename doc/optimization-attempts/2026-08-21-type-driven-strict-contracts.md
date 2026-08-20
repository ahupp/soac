---
title: "Authenticated type-driven strict contracts"
---

# Authenticated type-driven strict contracts

- Status: interpreter enforcement in progress; optimization deferred
- Pacific date: 2026-08-21 PDT
- Change: `snluowlz`; initial worktree snapshot `dfbfc42f`, parent `4232685e`
- Outcome: complete interpreter enforcement and compatibility are pending;
  optimization and measurement require a separate request; no speedup claimed

## Current checkpoint — 2026-08-26 (PDT)

The latest complete `just test-all`, `field-contract-final-test-all-v5` at
`72b4e057`, finished at 2026-08-26 16:36:48 PDT with exit status 1 after
9,027.092811 seconds. The Rust and raw-runtime phases pass; 975 Python batches
pass and one times out, with no other failing batches
(`work/logs/field-contract-final-test-all-v5.json`). All 1,030 frozen inputs,
including both amended specifications and both submodule pins, remain unchanged;
source, selected runtime and selection postchecks pass. This is a complete
failing gate, not an enforcement/compatibility completion claim.

The sole timeout groups the CPython, compiled and entry nodes of
`test_reviewed_closed_pipelines_use_authenticated_entries` under one
300-second batch budget. Each node validates twelve authenticated modules
with its own actual checker fixture. Retained journals record twelve CPython
passes, twelve compiled passes and eight entry passes. The timeout stack finds
entry authenticating a dependency while importing the ninth module, not in an
iterator callback. This supports shared-budget exhaustion, but does not prove
the interrupted entry node completes without the targeted replay below. The
source analysis and retained journals are recorded at
`work/closed-pipeline-batch-budget-review-v1/evidence.json`.

A structured workflow regression reproduces the combined-backend grouping
before the scheduling fix (`work/logs/closed-pipeline-batch-budget-red-v1.json`,
23.470141 seconds, native/runtime/selection postchecks pass). The repair enrolls
only this twelve-module test in the existing reviewed-singleton policy. All
three backends, source bodies and validators remain intact; the 300-second
timeout, eight workers and compiler/runtime/enforcement behavior are unchanged.
The passing 275.32-second neighboring pipeline batch is documented but not
rescheduled.

The subsequent workflow and closed-pipeline family replay passes all 47 nodes
in 16 batches (`work/logs/closed-pipeline-batch-budget-green-v1.json`,
152.187687 seconds, native/runtime/selection postchecks pass). The previously
grouped backend nodes pass independently: CPython in 103.97 seconds, compiled
in 121.21 seconds and entry in 136.63 seconds. The unchanged neighboring batch
also passes in 138.01 seconds. Source/validator hashes remain unchanged:
`tests/test_closed_iterator_pipeline.py` is
`bd4ae85274354c94224b3ff308567aaf04373d333dc5b3f18348b577ad4b4e92`, and
`tests/fixtures/strict_closed_pipeline_cases.json` is
`75218c1ed67d350a110482f1812a87f6a6d33715530ea97983dee7187b9a70f8`.
This targeted GREEN result is separate from the original v5 failure. Per the
user's explicit instruction, no further whole-suite `just test-all` is planned;
the failed batch and its focused scheduling coverage have been replayed while
preserving the original failed receipt. The original full gate is not reported
as green. Optimization and benchmarks remain deferred, with all changes local.

The preceding full gate, `field-contract-final-test-all-v4` at `973baf54`, was
interrupted before completion; the VM was subsequently found stopped. Its
retained log reports 1,892 workspace Rust passes, 11 raw-runtime passes,
537 passing Python batches and 25 timed-out batches. No original terminal
receipt or complete postcheck is available. After restarting the VM, all
1,030 frozen inputs, both pins and the selected native runtime identity
verify unchanged (`work/logs/field-contract-v4-recovery-v1.json` and
`work/logs/field-contract-final-test-all-v4.recovery.json`). That recovery
audit is not a replacement full-suite pass. Early retained stacks show
checker source-byte verification, input hashing and fixture-build-lock
waits; guest-kernel soft-lockup warnings begin at 12:46 PDT, after the first
test timeouts, so the initial slowdown's cause remains unproven.

A separate diagnostic defect discarded partial GDB output: `TimeoutExpired`
can retain bytes despite `text=True`, and the timeout handler concatenated
them with strings. The focused regression reproduces that exact error
(`work/logs/timeout-stack-bytes-red-v1.json`). Decoding each captured stream
independently, with replacement for incomplete UTF-8, preserves diagnostic
output without changing timeout or cleanup policy. All 14 workflow tests
pass (`work/logs/timeout-stack-bytes-green-v1.json`), including the ten stream
combinations, original timeout status and worker cleanup. All 100 cases from
the 25 timed-out batches subsequently pass under the original eight-worker,
300-second policy (`work/logs/field-contract-v4-timeout-replay-v1.json`,
499.355522 seconds, source/runtime/selection postchecks pass). The replay
records read-only guest pressure samples at
`work/logs/field-contract-v4-timeout-replay-health-v1.jsonl`; no runtime or
test-semantics change was needed for these cases. That replay was followed
by v5, recorded above; the VM interruption's initial cause remains
unestablished.

The additional traceback/frame-inspection amendment supersedes the retained
frame-projection work below. The working implementation removes synthetic
SOAC frames, source-parent frame links, locals-plus projections, source-error
sites and normal-function native-region admission. Shared exception/operand
cleanup, lexical cells, class-source authentication and storage checks remain.
The subsequent observer amendment also excludes mandatory observer refusal.
The intermediate borrowed observer scope is removed along with existing
observer-only reservations, setter interception and fallback gates. The isolated
native frame-removal logical commit `c1ad25b5` and generated-only top `377da338`
passed development linking and C/C++ smoke, but are superseded, unpromoted
evidence: that candidate still implemented the now-excluded observer policy.
The observer-only removal is local logical commit `2504cd78`, a child of
`c1ad25b5`; its source audit finds no remaining deleted observer symbols or
include/Makefile references. The ordinary observer setters are restored;
custom-evaluator guards remain only for actual source/generated-code
authentication and pending construction completion. The runtime selection
remained unchanged during isolated candidate validation. Generated
top `6f899ed4` passes regeneration/check and isolated development linking:
115 native modules checked, no failed imports, removed exports absent and
retained field/construction exports present. The build receipt is
`work/cpython-no-soac-observers/development-v1/complete.json`. Public C++
header smoke passes; the first C fixture smoke caught an implicit dependency
on `pycore_interp.h` in the retained wrap-allocation-failure helper. Its explicit
include is repaired and the fresh C/C++ smoke passes
(`work/cpython-no-soac-observers/native-smoke-v2/complete.json`); failed v1
inputs/logs remain retained. The full 521-case native cohort passes with
520 passes and the existing `Py_REF_DEBUG`-only skip
(`work/cpython-no-soac-observers/native-focused-v4/complete.json`); source,
fixture, runtime and selection postchecks pass. Earlier attempts found a
nested-child fixture import-path error and one remaining mixed implicit-
finalizer timing comparison. The runner now gives nested children the frozen
fixture cwd; the test preserves ordinary controls and checks managed terminal
protocols at quiescence, owner barriers, reentry safety and exactly-once cleanup.
Neither repair changed native source or relaxed field enforcement.
The earlier `377da338` cohort replay was explicitly cancelled on the observer
scope change; no complete cohort or postcheck pass is claimed for that replay.
Full compatibility remains pending.
BlockPy cache 53 invalidated the removed IR metadata; cache 54 invalidated source
builtin snapshots, cache 55 corrects lambda-default binding, cache 56
retires class-region slot/snapshot IR, and cache 57 fixes compiler-created
set construction, as described below. No optimization or
benchmark work is authorized by these removals.

The first observer-removal promotion recorded `6f899ed4` in the actual JJ Git tree and shared
`vendor/cpython` checkout (`work/no-soac-observers-promotion-v2/complete.json`).
The first promotion preflight stopped before any source mutation because the
isolated Git environment lacked an author identity; the retry used verified
identity variables only for that command. Ruff remains `52ce33a9`, the protected
runtime-state design document is unchanged, and the saved runtime selection
was not silently restamped. The fresh matching optimized PGO/LTO build passes
(`work/no-soac-observers-promotion-v2/optimized-v1/terminal.json`), including
source, tool-input, selection and actual executable/library postchecks. It is
now selected normally, and the new runtime identity is frozen in
`work/no-soac-observers-promotion-v2/expected-runtime-v1.json`. The matching
SOAC extension is rebuilt and staged
(`work/logs/observer-removal-stage-runtime-v1.json`, identity postchecks pass)
before Rust or SOAC runtime validation. No tests used the previous runtime
against the promoted native headers.

All nine affected Rust crate test targets type-check after both removals and
the semantic repairs on the new selected runtime
(`work/logs/observer-removal-rust-check-v9.json`, source/runtime identity
postchecks pass). This compile-only result does not itself validate the new
ABI at runtime. Earlier check failures caught deleted-section callers, a shared test
constant, a validator initializer, stale test imports and a stray test-only
attribute on required GC traversal; those were repaired before runtime testing.
The 13 workflow-only tests pass (`work/logs/observer-removal-workflow-v1.log`),
including the per-case nqueens timeout policy; no timeout or workload was
relaxed. All 14 split Python test files pass a guest syntax check. The
field-only source audit finds call plans carrying source identity and arity,
not argument/return proofs; method/optional-field results remain unknown and
actual field/method capabilities still require native sealed storage/type
witnesses. These source checks do not replace matched-runtime behavioral tests.

The nine defining field-only cases pass on the newly selected runtime across
compiled SOAC, the entry interpreter and CPython
(`work/logs/observer-removal-field-only-v1.json`). The final source audit found
one unused cleanup-block field and a frame-admission preflight called only by
a test; both are removed. The test still exercises the actual deopt entrypoint,
pending-error preservation and complete owned-buffer release. After restaging
the extension (`work/logs/observer-removal-stage-runtime-v2.json`), all ten
focused native/Rust field, recursion-ABI and exception/ownership regressions
pass (`work/logs/observer-removal-native-rust-v1.json`). All runtime identity
postchecks pass. The two legacy generator fixtures have separately reproduced
failures: they imported ordinary sources while asserting old SOAC wrapper
internals (`work/logs/observer-removal-legacy-generators-red-v1.json`). Their
migration must prove authenticated source execution and separate ordinary
helper controls; the original failures are not expected-failure annotations.
The migrated template and all four corrected StopIteration scenarios pass
(`work/logs/observer-removal-generator-fixtures-v1.json`); the latter fixture
now uses the existing `PyFunction_GetSoacStrictOwner` export rather than a
nonexistent spelling. The 42-case semantic replay passes 32 cases and fails
ten (`work/logs/observer-removal-semantic-cohort-v1.json`): those four fixture
failures plus six real async-comprehension failures in compiled/entry execution.
Zero-capture compiler-created coroutine helpers incorrectly received an
ordinary-function code template; the native family check correctly rejected
it. The narrow repair reuses the existing suspended-family code factory,
preserving original code preference and native authentication. A structured
four-family regression is added. After staging the matching extension
(`work/logs/observer-removal-stage-runtime-v3.json`), all three focused family,
original-code identity and private-helper ownership Rust regressions pass
(`work/logs/observer-removal-helper-family-rust-v1.json`). All eight async
comprehension replay cases pass across compiled and entry execution
(`work/logs/observer-removal-async-and-guarded-v1.json`); that mixed run still
fails its separate guarded-builtin fixture as described next. Runtime identity
postchecks pass. No native guard is weakened or frame reconstruction reintroduced.

The authenticated guarded-builtin fixture exposed a separate semantic defect:
an initially absent module name was compiled as a fixed builtin even though
strict modules permit its first binding later. A minimal behavioral regression
preserves lookup-before-argument-callback order, actual opaque return values,
attribute/dictionary/C-API first bindings and subsequent permanent-binding
protection. Its unchanged CPython control passes while compiled, profiled and
entry SOAC fail (`work/logs/strict-builtin-shadow-red-v1.json`). The repair keeps
strict source builtin reads in the existing indexed-global path, which checks
the live module then the function's actual captured builtins. Compiler-created
runtime helpers remain explicit and unaffected. A structured lowering test
checks both properties; cache 54 rejects artifacts containing old snapshots.
The matching extension is staged
(`work/logs/observer-removal-stage-runtime-v4.json`), the structured regression
passes (`work/logs/strict-builtin-shadow-rust-v1.json`), and all 16 driver unit
tests pass (`work/logs/strict-builtin-shadow-driver-v1.json`). The initial driver
filter matched no tests; only the latter unfiltered run supplies driver evidence.
The behavioral replay passes all seven selected tests, including all four
backend/mode variants, both 19-case authenticated precondition matrices and
the guarded generator's existing profile/verify/apply replay
(`work/logs/strict-builtin-shadow-green-v1.json`). Runtime/source postchecks
pass. The original sources and validators remain intact; only the documented
permanent-binding differences are expected rejections.

Generated lockfile, inventory and dependency-graph evidence is refreshed at
`work/generated-input-refresh-20260826-v1/complete.json`; all six commands and
runtime identity postchecks pass
(`work/logs/observer-removal-generated-inputs-v1.json`). The older receipt
predated one legitimate test-only `soac_opt` dependency. The fresh commands
leave current generated bytes and external dependency versions unchanged;
the old receipt is preserved rather than edited to match.

The previous `just test-all` was cancelled when the two specification files
changed, and its input postcheck correctly rejected the changed root revision.
`work/logs/field-only-test-all-v1.json` records 2,539.176261 seconds, command
status 101 and failed input consistency. The root delta at cancellation was
exactly `OPT_GOAL.md` and `doc/TYPE_DRIVEN_OPTIMIZATION.md`. This is an
interrupted, invalidated gate, not a full-suite result. Its two independent
Rust failures (plus 140 poisoned-lock cascades), shared-timeout nqueens batch,
and frame-only comprehension admission failures remain diagnostic evidence.
The field-write oracle and stale native wire assertions are repaired; heavy
multi-phase nqueens tests now receive separate unchanged timeout budgets.
A new full gate is required after both removals and the matched-runtime build.

The fresh `just test-all` run at `c8413558` froze 1,031 tracked inputs, including
both amended specifications and the exact native/checker pins
(`work/logs/observer-removal-test-all-v1.inputs.json`). The build and all eleven
raw-runtime tests pass. Workspace Rust tests report 1,879 passes and nine
failures: six JIT tests share an obsolete same-block deopt-return assertion,
two lowering tests locate ordinary generators through optional strict-only
metadata, and the remaining lowering failure exposes missing containing-scope
lambda-default assignments. Actual deopt completion still passes through
handled-state finish and terminal-root cleanup; the test-only helper must
follow that CFG without requiring native frames. The lambda failure is a real
semantic defect: two name collectors skip defaults along with the lambda body.
Repair only default traversal and retain body-local isolation, enclosing-scope
assignment and comprehension-capture assertions.

The already-failed gate was stopped through the pytest runner's own cancellation
path before changing tracked inputs. At that point 132 Python batches had
passed with no failed batch; this is partial coverage, not a full Python pass.
All 24 owned processes stop cleanly
(`work/logs/observer-removal-test-all-v1-cancel.json`). The gate receipt records
674.976797 seconds and status 101, with source, runtime and all frozen-input
postchecks passing (`work/logs/observer-removal-test-all-v1.json`). A new
authenticated lambda-default regression is added before the production repair.
Its ordinary and authenticated CPython controls pass; SOAC fails during actual
lowering because a lambda default's enclosing captured cell is missing
(`work/logs/lambda-default-scope-red-v1.json`, runtime/source postchecks pass).
The two collectors now visit parameter defaults without entering lambda bodies;
the original unit subjects retain their binding assertions with additional
structured store/capture checks. Cache 55 rejects old lowered bindings.
After staging the repair (`work/logs/lambda-default-scope-stage-v1.json`), all
nine Rust crate test targets type-check
(`work/logs/lambda-default-scope-check-v1.json`). The complete lowering,
driver and JIT unit suites pass all 1,414 tests, including all nine formerly
failing cases (`work/logs/lambda-default-scope-rust-v1.json`); source/runtime
postchecks pass. All 17 cases in the authenticated behavioral replay pass
(`work/logs/lambda-default-scope-green-v1.json`), including compiled/entry/CPython
lambda-default order and capture semantics, comprehension cleanup and all nine
field-only boundary controls. A fresh complete gate remains required.

The next full gate at `3547b4eb` freezes the same 1,031 tracked inputs
(`work/logs/observer-removal-test-all-v2.inputs.json`). All 1,889 workspace
Rust tests and eleven raw-runtime tests pass. The Python stage reaches 168
passing batches and six failing batches before controlled cancellation: three
class comprehensions hit a remaining compulsory native-slot projection gate,
and three dynamic-code cases still assert the superseded frame-sensitive error
message. The latter requires separating dynamic source authority from excluded
frame inspection; it is not permission to accept arbitrary inherited strict
code. All thirty owned processes stop cleanly
(`work/logs/observer-removal-test-all-v2-cancel.json`). The receipt records
1,255.138357 seconds and status 143, with unchanged tracked inputs and passing
native-source, selected-runtime and binary postchecks
(`work/logs/observer-removal-test-all-v2.json`). This remains a failed,
incomplete gate, not a full-suite pass.

The follow-up removes class-region save/clear/restore operations, snapshot
owners, native-slot coverage and unwind-floor plumbing from the compiler,
optimizer consumers and both SOAC execution engines. Class comprehensions use
the existing lexical helper lowering. Actual class namespace/cell ownership,
source-bound captures and construction authentication remain required. The
test split retains ordinary CPython frame controls and in-scope namespace,
capture, exception and cleanup assertions. A separate audit finds that the
contextual dynamic-code guard also rejects documented ordinary explicit-code
execution and `compile(..., dont_inherit=True)` before normal call binding;
its repair must carry captured builtins explicitly and preserve strict-code
authentication without consulting an unrelated native frame. These changes
are in progress and have not yet been validated by a fresh full gate.

The class-cell removal passes nine-crate test-target checking and twenty-two
focused core, native-decoder and canonical-binding tests on selected `6f899ed4`
(`work/logs/class-semantic-rust-check-v1.json` and
`work/logs/class-semantic-binding-families-v1.json`). A new delayed-`__class__`
fixture first fails ordinary `compile` and authenticated Details alike, before
SOAC lowering (`work/logs/class-semantic-classcell-control-v1.json`). The
relevant symbol-table/closure construction matches recorded pre-contract base
`b607563d`; pristine upstream behavior is not claimed. The failure remains
retained evidence. A separate positive source establishes and exercises a real
`__class__` method as well as the delayed lambdas; ordinary and strict compilation
then succeed, while Details correctly refuses a source without strict opt-in
(`work/logs/class-semantic-classcell-method-control-v1.json`). This still exercises
actual class-cell sharing, not a frame-layout surrogate. Its SOAC replay exposed
a genuine generated-cell alias mismatch. The repair connects the generated
class-cell alias to the actual selected class-header cell, preserving distinct
lexical/free-cell captures. On `6f899ed4`, the combined focused Rust results are
**55 passes**: the 22 binding tests above, 32 strict-source tests and one exact
native-capture test. The original four class integration programs pass their
16 backend/entry replays. The added positive source initially fails offline `ty`
admission for `__class__` in a delayed lambda inside an eager comprehension;
this is not classified as an intentional rejection. The isolated checker repair
must follow eager comprehension parents to the actual class without bypassing
an intervening explicit binding. The first isolated mdtest launch failed before
tests because a root `soac_source` patch introduced a second Ruff path-crate
family. A byte-identical isolated source bridge now resolves its dependencies
against the staged checker family; selected sources and checker identity are
not changed to bypass that failure. The real semantic RED then identifies five
delayed captures; the narrow repair passes the existing class-cell file and the
ten-file scope family, including explicit shadowing and eager-read controls.
Required scoped hooks pass after installing the pinned formatter. Their five
ignored cache files are moved intact outside the checker source, not ignored
by the committed-source verifier. The final cache-free scope replay passes
10/10, with source and dependency-lock postchecks
(`work/logs/ruff-class-cell-scopes-green-v3.json`).

The actual local Ruff commit is `3a5be884`, a two-file child of `52ce33a9`.
The reviewed diff contains the lexical parent walk and the existing mdtest's
regressions/prose/formatting only; no runtime type-call checks or frame machinery
are added. It is pinned in SOAC's actual JJ Git tree and shared vendor checkout
(`work/ruff-comprehension-class-cell/promotion-v1/complete.json`). Native source,
the selected runtime, protected document and unrelated root files/user refs are
unchanged. New JJ keep refs are recorded as internal bookkeeping, not mistaken
for user-branch changes. All required local objects are present; neither a push
nor complete upstream history is claimed.

The source-builtin repair also exposes a compiler-inserted `set()` call in eager
helper initialization. A legal later `module.set` binding redirects that call,
unlike a Python set comprehension. The behavioral baseline passes CPython and
fails SOAC with the shadow callable's exact assertion
(`work/logs/set-constructor-shadow-red-v2.json`). Version 1 selected the wrong
fixture module and is retained as a harness failure, not semantic evidence.
The narrow repair uses an empty set AST node and existing literal lowering;
it introduces no optimization, guard or new runtime helper. The corrected
behavioral replay passes both backend nodes, including the compiled and
entry-interpreter executions and their ordinary controls
(`work/logs/set-constructor-shadow-green-v1.json`); source/runtime postchecks pass.

The explicit ordinary dynamic-code repair is local CPython logical commit
`7181ac26`, with separately generated top `2624902d`. Its isolated development
build links and checks 115 modules with no failed imports
(`work/cpython-explicit-dynamic-context/development-v1/complete.json`). The
ordinary builtin implementations remain shared; SOAC transports actual captured
builtins, preserves ordinary binding/conversion and post-audit namespace lookup,
and rejects inherited strict execution authority without requiring caller frames.
This intermediate candidate is unpromoted and is not a compatibility pass.

A final consumer audit finds one remaining native restore vector, completeness
flag and derived wire boolean used only for the retired correspondence recipe
and its tests. Local logical commit `5c57eb2d` removes them; separately generated
top `7809e2b6` advances the native metadata wire from schema6 to schema7 and shrinks
regions from ten to eight fields. The real native comprehension restoration
body is unchanged apart from deleting the collector hook. Source and touched-
carrier data still authenticate actual class/annotation captures; no replacement
frame-retention proof is added. The four-file audit checks 1,426 native source,
header and build-input files for removed callers. This candidate is prepared in
another shared checkout, preserving `2624902d` and its build. Its development
build passes, with 115 checked modules and no failed imports
(`work/cpython-no-class-restore/development-v1/complete.json`). The actual C
fixture and public C++ header smoke pass against the new context ABI
(`work/cpython-explicit-dynamic-context/native-smoke-v2/complete.json`).

The final reduced-wire native cohort passes **521 cases with one existing
debug-only skip**, out of 522 collected cases
(`work/cpython-explicit-dynamic-context/native-focused-v2/complete.json`).
Source, frozen fixtures, runtime, provenance and prior-selection postchecks
pass. This includes storage-local field enforcement, pending/final construction,
generated-function metadata, ordinary CPython frame/observer controls and
explicit dynamic-code binding/audit behavior. It is a nondebug development
cohort, not a new StackRef-debug result or the full SOAC gate. The preceding
cohort stopped after 149 passes at one fixture's stale schema-6 assertion;
the exact failure remains retained. An AST audit found and updated that sole
direct version assertion while preserving all embedded fixture programs.

SOAC now records `7809e2b6` in the shared vendor checkout and its actual JJ Git
tree (`work/no-class-restore-promotion-v1/complete.json`); the subsequent checker
promotion records `3a5be884`. The 14 reviewed Rust context/wire consumer files and reduced-wire
Python assertion are applied. The old saved runtime selection was not restamped
or used with new headers. The fresh optimized PGO/LTO build passes, with actual
mode, source, selection and tool-input postchecks
(`work/no-class-restore-promotion-v1/optimized-v1/terminal.json`), and is selected
normally. Its runtime identity is frozen in
`work/no-class-restore-promotion-v1/expected-runtime-v1.json`. An independent
consumer review confirms context ABI argument order, captured-builtin ownership,
reduced-wire indices and preserved construction/field checks. A subsequent
test audit catches one stale Rust version-negative and structural corruptions
that would have short-circuited at the old version; the structural cases now
preserve the packet's actual version. The ordinary cache is invalidated by the
changed Rust build identity, while strict imports bypass that disk cache.

Two remaining function-local inspection refusal tails are removed from the
mixed call-context tests. Their actual module/class namespaces, ordinary
CPython callback controls, source admission, argument binding and effect order
remain covered. No new frame-inspection behavior is promised. The matching
extension is staged and all nine changed crate test targets check successfully
(`work/logs/field-contract-final-stage-runtime-v1.json` and
`work/logs/field-contract-final-rust-check-v1.json`). A final dead-code audit
removes unused generated-method and inherited-field accessors whose former
consumers were constructor parameter checks; actual field-state preparation,
binding and generated-code authentication remain intact. The matching extension
is restaged after that cleanup. All **37 offline-exporter tests** and **57
focused Rust regressions** pass on the final selected sources/runtime
(`work/logs/field-contract-final-ty-tests-v1.json` and
`work/logs/field-contract-final-rust-cases-v1.json`).

The first combined 58-node behavior replay passes field-write, ordinary-call,
dataclass, comprehension and framework cases, but five CPython context cases
fail at the same function-birth guard. The guard incorrectly requires a child
function's captured builtins to equal its parent's older capture, although
ordinary creation reads the current globals binding. A minimal nested-function
regression reproduces the failure before the fix
(`work/logs/field-contract-builtin-birth-red-v1.json`). Remove only that parent
equality; actual parent/code/globals authentication and the child's own captured
builtins snapshot/revalidation remain. No CPython source or pin change is needed.
The red replay and its successful runtime-identity postchecks are retained.
After staging the fix, the minimal positive/negative authentication regression
and all five original CPython context failures pass. The 36-node repair replay
also passes the revised expanded-argument test, frame-free capture/walrus/error
cleanup and explicit named-expression rejection/interoperability controls.
Its only three remaining failures concern the Enum test witness helper, which
rejects non-exact metaclasses before reaching runtime authentication
(`work/logs/field-contract-final-repairs-v1.json`). This is not a type-admission
or Enum execution result; the helper's own-namespace selection is being repaired
without dropping actual-function witnesses or invoking metaclass callbacks.

The 25-node delimiter replay passes all four original class-comprehension
programs across stock, compiled, entry and CPython backends, and the selected
dynamic-code/rejection controls. Three Enum selectors fail while analyzing an
unrelated shared-bank module, `named_expression_cases`. Its deferred method
creates a global that another scope reads without a module-level declaration;
`ty` deliberately retains that unresolved-reference error. The case is moved
to explicit strict rejection plus ordinary interoperability, preserving its
complete source and validator. Existing selected frame-free walrus/fibonacci
companions retain semantic coverage. No checker diagnostic is suppressed or
weakened, and the Enum/namespace cases remain selected positives. The original
failure is retained in `work/logs/field-contract-final-delimiters-v1.json`.

A remaining mixed expanded-argument test now compares source callback order,
exception context/identity and completed resource release separately from
implicit finalizer timing, order and observed context. Its ordinary CPython
schedule control remains intact. Current architecture prose no longer promises
native restore inventories, owned traceback source sites or observer-dependent
generator fallback. A final bounded generator-test audit also identifies mixed
frame-state, unused-local retention and traceback-root release assertions;
those are split while preserving original source programs, ordinary controls,
reentry/suspension safety and eventual cleanup. The final focused replay and a
fresh `just test-all` remain pending. No commits are pushed, and
optimization/benchmarks remain deferred.

The final focused replay passes **53/53 nodes in 17 batches**
(`work/logs/field-contract-final-generator-witness-v1.json`): all changed
generator validators and their ordinary controls, safe raw own-namespace
witnesses (including hostile metaclasses and spoofed instances), and actual
Enum execution in compiled, entry and CPython modes. The helper still requires
plain functions/exact wrappers and the original downstream source/owner checks;
it reads the known builtin type-dictionary descriptor without custom lookup.
All eleven original generator source/body constants remain unchanged. The JIT
test targets also type-check after the birth fix
(`work/logs/field-contract-final-rust-check-v2.json`). Final `cpython-info`
verifies the shared virtiofs source mount, recorded gitlink and selected
optimized executable/library (`work/logs/field-contract-final-native-info-v1.json`).
Host and guest space are checked before the full gate. A fresh `just test-all`
must now validate the frozen revised tracked inputs, including both user-amended
specification files; earlier interrupted/failed gates remain historical evidence.

The first fresh full gate on root `c7fec902` passes **1,882 top-level Rust
tests**, **11 raw-runtime tests** and **223 Python batches** before an orderly
cancellation for the final test-scope cleanup. There are no reported Python
failures or timeouts. The runner exits 143 after 2,106.973276 seconds, retaining
successful source, runtime, selection and all 1,030 tracked-input postchecks
(`work/logs/field-contract-final-test-all-v1.json`). All 25 owned processes are
verified gone (`work/logs/field-contract-final-test-all-v1.cancel-drained.json`).
This is retained partial validation, not a complete gate. Rust totals exclude
isolated child rerun summaries, which would otherwise double-count 45 tests.

The bounded production audit finds no remaining SOAC frame reconstruction,
slot-correspondence or observer-admission machinery. Actual source/function
ownership, suspended closure state, dataclass producer operands, exception and
recursion state, and the native managed-generator getter's allocation-safety
guard remain independently necessary. The test audit finds four remaining
inspection-only tails. SOAC generator frame probes and a locals-only reviewed
import invocation are retired; ordinary frame/locals controls and generator
identity, suspension, results and cleanup remain. Two implicit-eval cases now
name their independent explicit-globals restriction. Native context tests keep
explicit namespace identity, key callbacks, error propagation and temporary
ownership, without mandating missing-frame error behavior. Original source
programs and the reviewed JSON fixture remain unchanged. A focused replay and
then a fresh full gate validate this narrower final test set. The focused
replay passes **20/20 nodes in eight batches**, with matching runtime/source
postchecks (`work/logs/field-contract-scope-cleanup-focused-v1.json`); the
replacement full gate's result follows.

The replacement full gate on root `68ff4809` finishes at **2026-08-26 07:40 PDT**
with **1,882 top-level Rust tests and 11 raw-runtime tests passing**, and
**957 passing / 16 failing Python batches** from 3,499 collected nodeids
(`work/logs/field-contract-final-test-all-v2.json`). Its 1,030 tracked inputs,
native/checker pins, selected optimized runtime and actual binary hashes remain
unchanged; all postchecks pass. This is a failed gate, not compatibility
completion. Process-drain and repair-preimage verification is retained at
`work/field-contract-v2-repairs/verified-inputs-v1.json`.

One production defect is lambda helper placement: helper definitions escaped
the lambda body, losing parameters and lexical class-cell dependencies. Three
structured regressions first compile and fail for those actual scope errors
(`work/logs/lambda-body-scope-regressions-red-v1.json`). Private
`LoweredLambdaBody` syntax now keeps statement-bearing expression bodies in
their original lambda scope while defaults and creation remain at the original
expression site. Existing source identities, lexical binding and class-cell
visitors consume that syntax; no native frame, slot projection or lifecycle
proof is added. All **524 lowering tests pass**, including the three new tests
and existing default/class-cell/`super()` coverage
(`work/logs/lambda-body-scope-lowering-suite-v1.json`). Lowering and JIT test
targets also type-check, and the matching extension is rebuilt/staged before
runtime replay (`work/logs/field-contract-v2-repairs-stage-v1.json`).

The isolated optimization-case driver separately reproduces its missing
canonical helper import in a real authenticated `-I` subprocess
(`work/logs/opt-case-isolated-witness-red-v1.json`). Its repair uses the existing
validation prelude. Other reviewed repairs split stale frame/count/call-check
oracles from semantic and safety checks, correct invalid validators and actual
generator-factory witnesses, and align counter selection with actual source
targets. Their runtime replay remains in progress; successfully reaching a
later assertion is not a passing case. Existing profile/verify/apply tests are
compatibility replays, not new optimization or benchmark work.

An initial code-replacement diagnosis proposed accepting ordinary replacement
bodies at final source sealing. That proposal contradicted the existing
kept-preseal rejection and terminal-owner tests. All three production drafts
were rejected without application. The corrected lifecycle test preserves
ordinary preseal execution, original-source seal rejection, absent module
publication and escaped-owner failure; it does not restore argument/return
checks or expand admission. Workflow improvement: include the neighboring
admission/terminal-ownership controls before treating a lifecycle failure as a
new policy requirement. A fresh complete `just test-all` is still required.

The first repaired runtime cohort executes **92 nodeids in 30 batches**:
**25 batches pass and five fail**
(`work/logs/field-contract-v2-repairs-focused-v1.json`). The original lambda,
async suspension/cancellation, ordinary callback, mutable-referent field,
preseal/terminal-owner, native count/GC, late-owner field, hot-loop and
StopIteration replays pass their reached semantic/structured checks. Remaining
failures include two newly introduced ordinary constructor controls that
omitted `textwrap.dedent`, two stale unchecked-target/index-hit expectations,
and a real existing verify-path exact-int branch-plan attachment error. The
constructor repair changes only the two ordinary source-loading call sites;
counter-tail repairs preserve complete executable sources and validators.
The branch-plan source and counter assertions are not weakened.

Two other batches stop at checker source preflight, not their test bodies:
Jujutsu returns 255 with `Failed to read checkout state` / `ENOENT` during
initial and post-build pin verification. Both complete original diagnostics
and hashes are retained in
`work/field-contract-v2-jj-query-diagnostics-v1/followup-v1.json`. A read-only
follow-up succeeds; it does not establish either failure's cause. No index
fallback or retry policy is added. The recorded working revision advanced
while tracked edits were being made without an explicit agent snapshot, so
future cohorts keep documentation edits outside their execution window too.
This workflow constraint does not substitute for diagnosing any recurrence.

The next focused cohort passes the corrected constructor controls, canonical
isolated-driver regression, global-counter replay and all three cases previously
blocked by Jujutsu preflight. Its remaining failures are a stale positive
legacy direct-call fallback counter and a new two-mode lexical `super()`
regression (`work/logs/field-contract-v2-repair-followup-and-super-red-v1.json`).
The latter first passes its ordinary twin and actual source-owner witnesses,
then fails because a lambda in a nested definition's default has no class
cell. Two structural tests independently reproduce the missing lexical
normalization. The fix visits definition-time decorators/defaults in the
containing scope and gives every lambda body its own receiver context; all
**526 lowering tests pass**
(`work/logs/nested-default-super-lowering-suite-v1.json`). Runtime replay with
the rebuilt extension remains pending. No frame state or retention proof is
introduced.

The indexed-field verify failure separately has a structured RED at early
scalar-plan attachment: the matching guarded branch reports zero attachments
instead of one (`work/logs/indexed-field-scalar-plan-structural-red-v1.json`).
Its repair resolves existing field guards before callee snapshots and refreshes
remapped guards before scalar-dependent linearization. Early attachment and
linearization share one exact expression matcher, exported as
`soac_opt::passes::typed_exact_int_region_matches_field_expression`; final
all-input guard and missing/colliding-plan validators are unchanged. The
original Python source and counter assertions remain intact. This existing
compatibility-path repair is not a new optimization or performance claim;
both changed crates' test targets pass, and the matching extension is rebuilt
and staged. The first complete JIT suite passes **877 tests**, including the
new regression, with one stale mixed-test failure
(`work/logs/indexed-field-scalar-repair-rust-suites-v1.json`). Its old assertion
required a selected matching ordinary indexed region to be split. The test
now retains generic and unselected indexed-field negatives; existing
mismatched-owner/index validation also explicitly checks normal linearization.
The new regression separately requires selected matching regions to survive.
The optimizer suite was not reached after that JIT failure. Complete reruns
then pass **878 JIT and 248 optimizer tests**, including the selected, unselected,
parameter-only and mismatched-guard cases
(`work/logs/indexed-field-scalar-repair-rust-suites-v2.json`). Actual Python
replay remains pending.

Actual decoding of the retained direct-call profile and verify dumps finds
80 authenticated hot-target samples and zero counts for both legacy direct
branches (`work/logs/opt-direct-counter-decode-v1.json`). Strict-source target
filtering removes the unchecked guard before emission; ordinary contextual
vectorcall does not increment that guard's fallback branch. The earlier
positive-fallback prediction and a subsequently suggested owned-operand route
were incorrect for this fixture. Only the final counter-tail minimum changes
to zero; its full executable/validator prefix is byte-identical. Workflow
improvement: use decoded counter rows together with the actual plan filter
before predicting which emission branch ran.

The repaired authenticated replay now passes **102 nodeids in all 33 batches**
(`work/logs/field-contract-v2-repairs-focused-v2.json`), including the unchanged
indexed-field verify fixture, direct-call counter correction, both new
nested-default `super()` modes, original lambda/class-cell/default scopes,
ordinary controls, async suspension/cancellation, field writes, preseal and
terminal ownership, and the existing profile/verify/apply controls. All native
source/runtime/selection postchecks pass. This is focused compatibility
evidence; a fresh complete `just test-all` on the repaired frozen root remains
required. The old failed gate and all intermediate failures are retained.

The third full gate on root `4f7b4e9d` is cancelled after **1,888 top-level
Rust tests, 11 raw-runtime tests and 60 Python batches pass**, with no reported
test failure (`work/logs/field-contract-final-test-all-v3.json`). This is not a
completed gate. Independent source review finds that the field-layout resolver
used by the phase-order repair can construct temporary instances and invoke
inherited initialization, attribute hooks and finalization. It also identifies
an owned-prefix cleanup risk when a preserved nested field expression takes a
generic failure path. Neither finding is established by the prior passing
focused tests; behavioral regressions are being added before production fixes.
The verified runner is terminated through its normal worker-drain handler; all
22 owned processes exit, and all 1,030 frozen input, source, selection and
binary postchecks pass. Both amended specification files are included. No
tracked file is edited during this gate. Workflow improvement: field-plan
resolution must be callback-free, and nested-input failure cleanup needs its
own semantic regression rather than relying only on plan-shape assertions.

The first two authenticated callback/cleanup probes both pass; they are not
reproductions (`work/logs/field-planning-safety-regressions-red-v1.json`). The
two-field replay explicitly declines both missing field guards. An imported
ordinary receiver probe initially fails only its missing-profile-layout
assertion: arbitrary ordinary types are not automatically watched. The revised
source-created, automatically fallback subclass passes actual profile/layout,
verify and apply checks (`work/logs/field-planning-fallback-owner-red-v1.json`).
No profile is manufactured and none of these passes proves the suspect paths.

The actual `SpecializationProfile` resolver unit independently reproduces two
rounds of inherited initialization, custom setters, descriptor setting and
finalization while merely resolving a field proposal
(`work/logs/field-planning-map-callback-native-red-v1.json`). A separate
executable native JIT test retains nested getters through real preparation and
emission: success releases the payload, but a raising RHS leaks the owned LHS
after exception identity/order checks and after traceback roots are cleared
(`work/logs/nested-field-getter-native-cleanup-red-v2.json`, `fail=true`).
The cleanup repair reuses existing owned-input failure continuations and local
fallback, excluding borrowed local owners whose transfer has not committed.
It adds no frame retention or new ownership proof system. The identical
executable regression now passes both success and RHS-error cleanup
(`work/logs/nested-field-getter-native-cleanup-green-v1.json`). The companion
structured test also passes final partial-guard invalidation, preserved source
getter identities, actual ownership planning and the explicit-local
linearization control
(`work/logs/partial-field-guard-invalidation-structure-v1.json`). It establishes
the accepted nested shape, not a new requirement that early matching use all
guards. An initial test draft imposing that unsupported predicate was rejected
unapplied. All **20 existing operand/transfer/deoptimization controls** also
pass with the cleanup fix (`work/logs/owned-prefix-operand-controls-v1.json`),
including borrowed local transfers, failed siblings and suspended cleanup.
Field resolution now scans actual module/type namespaces and MRO dictionaries
without Python lookup/equality callbacks, holds the resolved owner while
deriving its guard, and never allocates sample instances or writes sample
fields. Normal indexed reads, stores and scalar inputs reuse the existing
current-key/name guard; an expected index alone is not a storage guarantee.
The obsolete public `IndexedFieldLayoutGroup` and
`indexed_field_layout_groups` priming API is removed. Native ABI, artifact
schemas, protected-write barriers and general symbol resolution are unchanged.
Both affected crates' test targets type-check after correcting a missing
`PyModuleMethods` import in the new test, and the matching extension is staged
(`work/logs/field-planning-safety-repair-test-targets-v2.json`,
`work/logs/field-planning-safety-repair-stage-v1.json`).

The first complete repaired JIT suite passes **880 tests**, including both
reproducing regressions, malformed/callback namespace declines and an actual
same-owner wrong-key/index native fallback. One older positive remapping test
fails because its fixture removes the owner module before resolving a raw
registered address (`work/logs/field-planning-safety-repair-rust-suites-v1.json`).
The fixture now retains its actual namespace through the same positive
inlining/remapping assertions. The new independent negative case still requires
an unanchored registry address to decline. The optimizer suite is not reached
after the JIT failure. The corrected complete replay now passes **881 JIT and
247 optimizer tests**, with all native source/build/selection postchecks
passing (`work/logs/field-planning-safety-repair-rust-suites-v2.json`).
Independent source review then identifies an unproven pre-existing boundary:
ordinary-storage checked-field owners can retain inline values, while the
normal indexed store has no explicit field-policy barrier. A targeted
authenticated profile/verify/apply regression is being prepared; this is a
source concern, not an observed bypass or a completed full-gate result.
The first new authenticated inline-storage replay passes unchanged
(`work/logs/checked-inline-field-store-red-v1.json`, 90.875188 seconds, all
native postchecks pass). Its retained source/site counters show **2,000 generic
profile stores and 4,004 generic verify stores**, with zero indexed hits or
fallbacks. It preserves actual field rejection, an ordinary subclass's
inherited policy, an ordinary twin and cleanup without requesting the receiver
dictionary. This is generic-path compatibility evidence, not an indexed-store
reproduction. The companion raw object-slot resolver also needs to decline
installed slot policies; both proposed declines use existing callback-free
native queries rather than restoring call checks or adding a write protocol.
The parameterized inline/source-requested-slot replay also passes before the
fix (`work/logs/checked-inline-slot-field-store-red-v1.json`). Both write sites
remain generic; the slot reader records three indexed hits and three fallbacks.
Two focused Rust tests then execute the actual resolver boundaries against
native own/inherited policy fixtures and unchecked controls: both initially
fail by admitting a protected owner
(`work/logs/checked-field-policy-eligibility-red-v1.json`). These are compiler
eligibility failures, not evidence of an observed public-source write bypass.

The applied repair declines normal indexed-field guards and own/inherited
late-bound publication for native ordinary-dictionary policies. The shared raw
slot-offset resolver declines native slot policies, including its generator-
factory consumer. It uses existing exported native queries, preserves the
generic checked path, and adds no native ABI or public Rust API. After package
formatting, changed-crate test-target checking and matching extension staging,
all **883 JIT and 247 optimizer tests pass**, including the two new eligibility
regressions (`work/logs/checked-field-policy-test-targets-v1.json`,
`work/logs/checked-field-policy-stage-v1.json`,
`work/logs/checked-field-policy-rust-suites-v1.json`). Native source, runtime and
selection postchecks pass throughout. The expanded authenticated Python replay
and a fresh frozen full gate remain required; no benchmark or performance
claim follows.
The expanded authenticated replay now passes **157 cases in 50 batches**, with
no failures (`work/logs/field-contract-safety-repairs-focused-v1.json`,
892.466699 seconds). It includes the complete checked-field family, ordinary
and source-requested-slot profile/verify/apply controls, late-bound fields,
unchanged indexed-field counter expectations, actual type/source ownership,
comprehension/exception/callback semantics and cleanup. Native source, runtime
and selection postchecks pass. No tracked inputs changed during the cohort.
The final bounded unused-helper audit found no additional named helper serving
only excluded frame or call-type enforcement: shared dataclass code-edge
authentication, source operand metadata and ordinary default-slot binding stay.
Test-only convenience accessors are not a retained SOAC frame executor. A fresh
`just test-all` on the complete frozen inputs is still required.

The storage audit also confirms a separate remaining representation boundary:
assigning a pre-existing ordinary dictionary to an instance preserves its real
identity, aliases and permanent field checks through the documented legacy
policy. It cannot acquire a body trailer in an already ordinary-sized
allocation. This path is explicitly unconverted, not a completed direct-state
migration. A later wording/source audit corrects the earlier inference that a
legacy exception is therefore necessary: the exact-dictionary requirement does
not explicitly require a body tail, unlike the adjacent list requirement. A
fresh private dictionary-payload tail could preserve object identity and
ordinary body size, but needs an explicit allocation/migration protocol and
copy, resize, clear, GC, OOM and freelist validation. Shared split/empty keys and
optional values cannot simply be repurposed. Do not reject replacement, copy
the dictionary, retag old memory or weaken installed checks. The current safe
legacy behavior is unchanged; the alternative has not been implemented or
validated, and this representation boundary remains open.

### Previous field-only checkpoint

SOAC locally pins CPython `1b0b2a46` and Ruff `52ce33a9` for the
2026-08-25 (PDT) removal of all function-level runtime type checks. The new
optimized interpreter is built, verified and selected. All nine affected Rust
crate test targets type-check, the matching extension is staged, and the nine
defining field-only behavior cases pass across three backends. A 240-case
compatibility run passes 230 cases and finds ten mixed observer-policy failures.
Those tests now separate CPython callbacks from explicit SOAC refusal and
untraced recovery; all nineteen focused observer/field cases pass across the
retained replays. `just test-all` remains required. Nothing is pushed.

The optional storage-state implementation is already present in `b8dcf1ca`;
its isolated development and actual StackRef-debug gates, matching optimized
build and earlier three-backend field tests are retained evidence, not proof
of the new field-only runtime. Schedule-only execution machinery is removed.
The sibling/nested-region and unattached scalar-plan defects found by earlier
compatibility runs have focused repairs. The two frame-local deletion fixtures
now separate unsupported local snapshots from callback, alias and cleanup
semantics; their matched-runtime replay passes in the 240-case cohort.
The chronological entries below retain intermediate failures and superseded
candidates; none substitutes for the final combined gate.

### Field-only runtime scope — 2026-08-25 (PDT)

Remove argument/return predicates from every backend and generated constructor,
including deferred factory-result checks. Preserve ordinary calls, authenticated
source ownership, metadata seals, pending/final type construction and selected
field-write predicates. An `InitVar` creates no storage obligation; constructor
effects preceding a forbidden field write still occur. Historical checked-call
and check-elision results below describe retired implementation, not retained
guarantees or future optimization authorization.

- Artifact schema 6 / strict contract 2 removes the four call-check policy keys
  and changes the manifest signature domain. Old keys, including disabled
  spellings, and old publications fail closed. The 59 contract unit tests pass
  (`work/logs/field-only-contracts-v2.json`); the initial run caught a duplicate
  test-only signing-domain constant, now replaced by the production constant.
  Static `ty` signatures and field provenance remain available without supplying
  runtime argument/result proofs. The shared predicate is now
  `StaticType::has_supported_value_shape`.
  The new checker pin passes all 37 wrapper/analysis/publication tests,
  including the three field-only policy tests
  (`work/logs/field-only-checker-full-v1.json`).
- Nine real-checker regression cases across compiled, entry and CPython paths
  demonstrate the old behavior being removed: annotation-only calls are rejected,
  constructor effects are suppressed before a field write, and a dataclass
  `InitVar` is checked. This intended RED is
  `work/logs/function-type-removal-red-v2.json`. Version 1 failed in the validator
  harness before those assertions; it is not semantic evidence. The source
  programs and corrected validators are unchanged in the matched green replay:
  **all nine pass** across compiled SOAC, entry interpretation and CPython
  (`work/logs/function-type-removal-green-v1.json`, runtime/source postchecks
  pass). Annotation-mismatched calls and returns execute ordinarily; preceding
  constructor effects occur and selected writes still reject incompatible
  values. `InitVar`, factory results and foreign-receiver initialization have
  no separate call-type predicate.
- Rust removes call-type plans, retained argument proofs, return helpers,
  check-only replay exclusions and dataclass deferred-value machinery.
  `TypedSourceCallPlan` retains independent actual-callee/body/arity guards;
  it grants no argument or return-type facts. Function diagnostics use schema 2;
  call statistics retain only actual native-body observations. BlockPy cache 52
  invalidates the retired serialized plans. Package-scoped formatting passes;
  all nine affected/dependent crate test targets type-check against the new
  interpreter (`work/logs/field-only-rust-check-v3.json`). The first check
  found three stale consumers of the removed API; the fixes preserve null
  rejection and source ownership. Unused call-check wrappers and the unused
  Rust frame-local accessor are removed after a caller audit.
  Unrelated pre-existing warnings are not suppressed or broadly refactored.
  The matching extension builds and stages successfully
  (`work/logs/field-only-extension-v1.json`). The first broader compatibility
  replay covers dataclasses, nominal bindings, actual class construction,
  framework fallback, ordinary defaults and local cleanup: **230 of 240 cases
  pass**, with source/runtime postchecks passing
  (`work/logs/field-only-compatibility-v1.json`). Ten failures in five tracing
  or profiling families occur at the documented SOAC observer refusal before
  their targeted callbacks. Split those mixed tests into genuine CPython
  observer/mutation coverage, explicit SOAC refusal and untraced semantic
  recovery; do not claim that refusal exercised the original mutation. The
  first split replay passes all seven nominal-slot cases, including compiled
  and entry checks of final `Self` field ownership through constructors,
  ordinary assignments, member descriptors and C APIs. Its other six passing
  cases and six failures give **13/19 passes**
  (`work/logs/field-only-observer-replay-v1.json`, postchecks pass). The failures
  are new validator mistakes: a direct-run driver lacks the repository import
  path for its shared CPython witness, and a nested-class repr oracle omits
  the qualified class name. Correct the helper setup and use the unchanged
  ordinary source as the repr control; no runtime change is justified by these
  failures. The corrected adapter replay passes **12/12 cases**
  (`work/logs/field-only-observer-replay-v2.json`, postchecks pass), completing
  the nineteen focused cases alongside the seven unchanged nominal-slot
  passes from version 1. Original source strings and the existing ordinary
  and CPython controls remain unchanged; application/source-audit receipts
  are under `work/dataclass-observer-policy-split-v1/`,
  `work/dataclass-nominal-observer-split-v1/`, and
  `work/dataclass-observer-helper-path-v1/`. The full gate remains pending.
- Native logical `bb33d8e` removes function type enforcement while leaving nine
  field/type-state source files byte-identical. The first generated candidate
  `3e66b796` links and passes C/C++ smoke, then its native gate finds a stale
  interpreter class-preparation ABI guard after 16 passing cases. Logical
  `2028dc90` fixes that guard; generated-only `1b0b2a46` is the selected source
  pin. Its fresh development build links, C/C++ smoke passes, and the frozen
  447-case cohort reports **446 passes and one existing debug-only skip**.
  This includes the 32-cycle ordinary/checked-dictionary reuse test and the
  retained generated-function construction, component, binding and cleanup
  cases. Source, fixture and runtime identities remain unchanged through the
  gate. Receipts are `work/cpython-field-only/development-v2/complete.json`,
  `native-smoke-v2/complete.json`, and `native-focused-v2/complete.json`.
  Both failed and passing candidates remain local; these isolated native
  results do not substitute for the combined optimized-runtime gate.
- The offline checker test exposed two vendored exporter calls to the renamed
  shared predicate. Local Ruff commit `52ce33a` updates those callers; hooks
  for the changed files pass, with the bare-rustfmt hook excluded under SOAC's
  package-formatting rule. No compatibility alias or checker patch is retained.
  Paired source/pin promotion and the complete checker test replay are verified.
- Mixed class/dataclass tests preserve source/creation witnesses, inherited
  field policies, method metadata seals, ordinary binding and callback order,
  C APIs, GC and pending/failed type barriers. They now permit annotation-
  mismatched calls and results. A complete escaped generated initializer from
  a failed Apply may run on a suitable ordinary receiver; that does not reopen
  the failed type's allocation barrier. Pure removed-call-check/proof tests are
  retired rather than converted to disabled assertions.

The coordinated local pin import changed only `vendor/cpython` and
`vendor/ruff` in the recorded JJ tree, preserving its change ID and parent,
all other entries, the protected runtime-state design document and old build
selection. Its first attempt stopped because sanitized Git had no author
identity. The narrow continuation uses the working revision's verified author
as per-command configuration, not global configuration. An intervening guest
JJ checkout-state `ENOENT` was recorded before retrying; no cause is asserted.
The already created carrier was resumed, verified and removed, not recreated
or pushed. Evidence is
`work/function-type-removal-promotion-v1/{commands.log,complete.json}`.
The fresh optimized build passes the normal recipe under two CPUs and
4 GiB/no-swap, with no OOM events. Its actual PGO/LTO, nondebug GIL executable
and loaded library match the new source; source/tool/manifest checks and the
prior selection remain unchanged throughout that build. The normal selection
recipe then selects it and the runtime runner freezes
`work/function-type-removal-promotion-v1/expected-runtime-v1.json`
(SHA-256 `6080256c07dd595842153cfef4ed72d92390d1651a8f865eb728ebbf2fb0133f`).
Build proof is `work/function-type-removal-promotion-v1/optimized-v1/terminal.json`.

Two attempts to run the new dictionary reuse test through the selected runtime
stopped before execution because guest Jujutsu could not read checkout state
(`ENOENT`). The exact command, return code and bounded output are retained; the
same query later succeeded both directly and with the verifier's sanitized
environment. The underlying cause is not established, and neither an index
fallback nor a weaker source check was used. The isolated candidate native test
subsequently passes that test. Optimization and benchmarks remain deferred.

### Execution-compatibility scope correction

The 2026-08-24 (PDT) clarification supersedes the exact-count and implicit
release-schedule acceptance statements in the historical checkpoints below.
SOAC must preserve source semantics, safe ownership, required cleanup,
supported inspection and installed contracts; matching CPython's transient
reference counts or fused-opcode schedules is no longer required.

- Before the clarification, the matched `3000af3e` optimized runtime passed
  21 focused Rust tests and 29 ownership observations. Receipts are
  `work/logs/coordinated-native-reference-units-v1.json`,
  `coordinated-native-reference-joins-v1.json`, and
  `coordinated-native-ownership-v2.json`. These preserve historical evidence,
  not a requirement to retain the schedule-matching implementation.
- Ten new public-vectorcall/same-code compatibility cases failed because the
  parallel token executor depended on a disposable native SourceEntry record;
  its two existing semantic controls passed
  (`work/logs/coordinated-native-entry-mutation-red-v1.json`). The proposed
  native continuation candidate stays local and unselected. Its development
  build verified, but its focused fixture did not compile, so no behavioral
  pass or debug gate is claimed.
- The separate token executor, typed token-body ABI/cache, entry-interpreter
  token storage and registration hooks are removed. Existing checked SOAC
  entries retain actual-owner authentication, once-only binding, parameter and
  return checks, captured invocation identity and exception-safe ownership.
  Exact scalar opcode recipes are removed and mixed tests now separate
  semantics/cleanup from native-only schedule observations. Preimages remain under
  `work/execution-compatibility-simplification/parallel-token-removal/`.
  Outgoing owned calls now borrow inputs through the existing contextual
  vectorcall and retire their own slots on either return; they do not transfer
  native stack references to reproduce a count. The runtime inventory and
  module-lifecycle descriptions now reflect that retained ownership behavior.
- Shared source-frame/traceback, monitoring, lexical scope and ownership
  components remain only for independent semantic or safety requirements.
  The native removal candidate is committed locally; focused replay is still
  pending. No new full-gate result is claimed. Optimization, benchmarks and
  remote publication remain deferred.
- Scalar recipes, eager-region protection/completion proofs and class-prefix
  opcode matching are now removed from the retained compiler path. Schema6
  retains original semantic scope/slot/owner/capture/access metadata. Its
  separate Store/CALL table stays for actual CPython publication and call-site
  authentication; only its unused read column is removed. Cache48 rejects old
  serialized SOAC projections. All eight changed Rust crate test targets
  type-check, including JIT/PyO3 and the metadata-safety changes
  (`work/logs/execution-compat-rust-v4.json`, status0, source/build postchecks
  passed). The build script now exports only the three independently required
  dataclass CALL IDs, not the discarded fused-opcode/cache-width inventory.
  This is not a runtime gate against the pending schema6 build.
- Opaque metadata reads check the actual owning destructor; an invocation
  captures owning template, compiler, module and body handles before callbacks.
  No new native token or metadata-freezing ABI is introduced. Twenty generated
  regression programs cover idle/binding/body metadata replacement, saved public
  entries, return checks, exceptions and cleanup; their guest syntax check
  passed, but behavioral replay is pending. Eight additional compiled/entry
  observer-refusal cases retain clean rejection, cleanup and follow-on checks.
- The unselected native candidate is `4f66b9d2`, generated-only above logical
  semantic-schema commit `84435a03` and runtime-removal commit `29066ddf`.
  Canonical source/index verification, local object closure/fsck and official
  regeneration passed. Its fixture migration removes eight token-only includes;
  45 original metadata-test source subjects and seven whole ordinary controls
  remain unchanged. Native builds, coordinated promotion and the matched
  optimized runtime gates are not yet claimed.
- Independent native review found a genuine reentrant metadata-setter leak,
  separate from the removed lifetime-schedule requirements. The selected
  `3000af3e` runtime failed all nine storage-control cases: six lose the nested
  payload without its destructor and three overwrite a nested clear. Source,
  owner, required-boundary and seal state stayed intact. The frozen source/build
  checks passed (`work/logs/metadata-reentrancy-red-v1.json`); these raw native
  storage controls are not authenticated strict-admission evidence. Logical
  `0b1ed371` publishes the replacement before retiring its predecessor and has
  an independent code review; behavioral green replay remains pending.
- The resulting `2080750a` candidate's first development build stopped on a
  deleted header include before any runtime tests. A complete deleted-file
  basename audit then found two stale includes and one stale Makefile
  prerequisite. Logical `730867c2` removes them and adds explicit dependencies
  for the retained observer component. The failed build, previous candidates and
  fixture postimages remain preserved; no stale build was selected or restamped.
- Its `9e477f5b` generated candidate passed compilation, then failed linking
  because the removal also deleted the shared `clear_gen_frame` and
  `_PyEval_FrameClearAndPop` definitions between two token-specific sections.
  These implement required ordinary frame/generator cleanup. Logical `f266924a`
  restores their complete block byte-for-byte from `6640`, independently checked;
  no retired token API is restored. The failed
  log is `work/logs/native-execution-compatibility-development-v3-build.log`;
  no runtime tests ran. The independent deletion audit found no further shared
  safety or enforcement omission. Its `1a61fb3c` generated-only candidate has
  passed source/index, object-store and regeneration checks. Its fresh
  development build verifies the actual executable/library, restored cleanup
  export and absence of retired token exports. The identical raw metadata
  reproducer now passes **9/9**, with source, fixture, build and prior-selection
  postchecks (`work/cpython-runtime-reference-removal/development-v4/raw/complete.json`).
  The final fixture replay passes **2/2 C/C++ smoke checks and 243/243 retained
  native cases**, with no skips, expected failures or errors
  (`work/cpython-runtime-reference-removal/development-v6/`). Two fixture-only
  failures were corrected and retained: the standalone C++ test now includes
  its own `<cstddef>`, and one ordinary paired-store control reads the wire6
  Store column at index4 rather than index5. Its original source and callback
  order assertions are unchanged; neither fix required a native rebuild.
- Local promotion completed **2026-08-24 23:25:35 PDT**. The actual JJ tree now
  pins CPython `1a61fb3c4948a4ce03053d342dffd8ac2c02c82a` and unchanged Ruff
  `e8a81e2f2f021b9fd34a003acd121bd6c6590130`. Five fixture updates and eight
  token-only fixture deletions were verified, with unrelated root bytes and
  the old saved selection preserved. Both sources remain on the same shared
  virtiofs mount. Evidence is
  `work/execution-compatibility-promotion-v4/manifest-v2.promotion/complete.json`.
  The old selected build was intentionally left stale until the fresh optimized
  build below. No source was restamped or remotely published.
- A separate fresh StackRef-debug build of the same `1a61fb3c` source passes
  **243/243 retained native cases and 2/2 C/C++ smoke checks**, with no skips or
  expected failures. Its actual debug ABI, executable, loaded library and
  source/build provenance were verified; no Rust/JIT code was run against the
  debug ABI. Evidence is
  `work/cpython-runtime-reference-removal/stackref-debug-v1/`. This checks safe
  native ownership, not CPython/SOAC transient-count equivalence. The selfdoc
  inventory and DOT/SVG graph regenerate consistently, as do the root and
  checker Cargo locks, with no byte or dependency changes
  (`work/execution-compatibility-validation/generated-docs-v1/` and
  `generated-cargo-locks-v1/`). Python lock regeneration and full runtime
  validation remain pending.
- The fresh `1a61fb3c` optimized build completed successfully at
  **2026-08-24 23:38:37 PDT**. The actual nondebug, GIL-enabled PGO/LTO executable
  and loaded library, committed source and build provenance were verified;
  `just build-python optimized --no-select` preserved the old selection
  (`work/execution-compatibility-validation/build-v1/terminal.json`). The build
  was subsequently selected at
  `/home/adamh.guest/.local/share/soac/builds/execution-compatibility-optimized-v1-01a02587`.
  Its matching SOAC extension build passes, followed by **1/1** native-recursion
  ABI test (`work/logs/execution-compat-extension-v1.json` and
  `work/logs/execution-compat-abi-v1.json`). These receipts verify unchanged
  selected source, executable/library hashes, build provenance and selection.
- The same selected optimized CPython passes all **six ordinary test modules**:
  `test_call`, `test_descr`, `test_types`, `test_dataclasses`, `test_generators`
  and `test_funcattrs`. The XML reports **851 cases, including two skips**, with
  zero unexpected failures or errors; the text log also records two declared
  expected failures in `test_descr`. The skips require the disabled CPU resource
  and a debug-only total-reference-count API. This isolated `-I -S` run uses ordinary
  CPython, without strict startup or SOAC import, and passes the same source,
  binary and selection postchecks (`work/logs/execution-compat-ordinary-cpython-six-v1.json`
  and its `.xml`). Authenticated checker/runtime, metadata and full-gate replay
  remain pending; these build and ordinary-compatibility results do not establish
  complete enforcement. Optimization, benchmarks and remote publication remain
  deferred.
- Matched-runtime Rust checks pass for all **three metadata-safety regressions**,
  **13 class/comprehension binding tests** and **nine source-frame ownership
  tests** (`work/logs/execution-compat-{metadata-probe,foreign-metadata,ordinary-metadata,class-bindings,source-frame}-v1.json`).
  The interpreter-source family first failed on an obsolete test-only scope
  column; its other 46 failures were poisoned-lock fallout, not independent
  semantic failures. Correcting three test-only column indices preserves the
  original subjects and assertions; the isolated failure and the complete
  **47/47** interpreter-source family now pass with unchanged native identity
  (`work/logs/execution-compat-interpreter-source-v2.json`).
  `just --command uv lock --project soac_py --python /home/adamh.guest/soac/.venv/bin/python`
  also reproduces the unchanged 21-package Python lock, with native identity
  postchecks (`work/logs/execution-compat-python-lock-v1.json`). All five root
  generated outputs now have fresh generation evidence for separate packaging.
- The first matched end-to-end sentinel run reports **15 passed, two failed**
  (`work/logs/execution-compat-enforcement-v1.json`). CPython-only authenticated
  loading, checked calls/returns, pending/final class admission, ordinary
  dataclasses, three framework fallbacks, warmed stores and C APIs pass. The two
  retained-backend slotted-dataclass callback cases expose an unterminated
  Cranelift entry block when emitting the new source-parent error path. An
  isolated replay confirms the codegen site; its fix and replay are pending
  (`work/logs/execution-compat-dataclass-cfg-red-v1.json`). No callback or
  contract assertion was weakened to accommodate the failure.
- The cold-entry block now emits after the body terminator, with the same
  failed-entry parameter binding, source-frame handoff and cleanup. A new
  structured test first reproduced the actual Cranelift panic and passes after
  the fix; **19 source-frame tests, one parent-scope test and both unchanged
  dataclass callback cases pass**. Scoped test-target checking, extension
  staging and frozen native checks also pass (`work/logs/dataclass-cfg-*-green-v1.json`).
  The first attempted unit fixture did not materialize an entry and was rejected
  by its own precondition; it is retained as fixture-development evidence, not
  a bug reproduction. Five new real-checker artifact-corruption cases are ready
  for replay: signature/version tampering, unsupported deployment version and
  missing/corrupt unrequested mandatory shards. They check first selected-module
  admission before module-body effects, not an earlier process-start boundary.
- The matched artifact/boundary replay passes **all 47 selected cases in 13
  passing batches**, with native source/build/selection postchecks
  (`work/logs/execution-compat-artifact-boundaries-v1.json`). This includes all
  five real-`ty` corruption/version cases, ten public-vectorcall/same-code
  compatibility cases, twenty metadata-replacement cases, eight observer
  refusals and four argument-cleanup cases. The broader compatibility repair
  replay and full `just test-all` are still pending. A residual audit also found
  generator tests imposing implicit exception-payload release schedules; those
  are being split without dropping explicit exception/control-flow or eventual
  cleanup coverage.
- The broader replay completed with **59 selected nodes in 20 passing and
  three failed batches**, with unchanged native source/build/selection
  (`work/logs/execution-compat-repairs-v1.json`). Its failures are a retained
  lambda/multiple-generator comprehension binding rejection, an indexed-field
  counter expectation requiring diagnosis, and a shutdown-counter fixture
  expecting the unselected ordinary `soac.runtime` dependency to be instrumented.
  Stock and CPython-backed execution pass the unchanged lambda source. The
  native semantic binding records contain its targets; the retained lowering
  consumer still restricts this path to one generator in an ordinary function.
  Repairs and focused replay are pending; no new optimization is authorized.
- The newly present **2026-08-25 (PDT)** storage-state amendment is an additional
  enforcement-representation requirement. The selected native `1a61fb3c` source
  has no `PyTypeState` implementation. Its existing enforcement results do not
  prove optional trailer allocation, a direct per-storage accessor or migration
  of existing policy paths. That implementation and its allocation/GC/escaped-
  dictionary validation remain explicit work before claiming the latest goal
  complete. Optimization and benchmarks remain deferred.
- The residual generator/shutdown replay selected **61 nodes in 17 passing and
  two failed batches** (`work/logs/execution-compat-generators-v1.json`). The
  split lifetime checks preserve original source, explicit exception/callback
  order, caller-handler restoration and eventual exactly-once cleanup; their
  ordinary controls retain the exact native observations. Those checks and the
  corrected shutdown fixture pass. The three failed nodes were two TaskGroup
  tests still requiring an obsolete projection refusal and a test claiming
  deoptimization despite recording no selected deopt. The TaskGroup tests now
  restore their actual post-GC frame-leak assertions. The Python handler case
  checks actual compiled execution, while the existing structured JIT deopt
  regression now verifies the actual active handler identity/context and caller
  restoration on its selected deoptimization path; that regression passes.
- The uniform-field investigation found no indexed plan: authenticated
  `CompleteFunctionDefinition` wraps the bare function form accepted by the
  existing optional late-owner catalog. This is not an installed-contract
  failure. The compatibility test retains its original source, checked-field
  semantics, ordinary controls and profile-layout evidence, but no longer
  demands an unselected optimization. The existing typed catalog regression
  explicitly rejects this unsupported shape while preserving its positive
  cases. No new indexed plan or guard-elimination work was added.
- The lambda binding consumer now accepts the actual lambda scope and multiple
  synchronous generator targets, including attributes; semantic source ranges,
  ordinals and original native names remain validated. The new public core
  `SourceRegionTarget` distinguishes local and attribute targets. Cache49
  rejects the previous serialized representation; native wire6 is unchanged.
  Five changed crate test targets type-check, the matching extension stages,
  and **20 focused Rust tests pass**, including the real deopt handoff, all 15
  class-binding checks and four existing field-plan/runtime controls. Root
  Cargo lock regeneration adds only the new `soac_opt` test dependency on
  `soac_contracts`, with no external version changes
  (`work/logs/lambda-region-lock-v1.json`).
- The next actual runtime cohort reports **46 passed, five failed**
  (`work/logs/lambda-region-python-compat-v1.json`). All 32 class-static cases,
  six eager source-frame cases, three TaskGroup cases, the handler case and the
  uniform-field case pass. Three of four new lambda cases pass; normal compiled
  execution returns a tuple whose restored outer-parameter element is null,
  causing a native crash. GDB on the unchanged authenticated fixture confirms
  this required ownership/dataflow bug (`work/logs/lambda-crash-gdb-v1.json`),
  not an allowed lifetime-schedule difference. The four other failures are
  old eager-control validator tails missing a `validate_module(module)` wrapper.
  Their source and assertions are preserved in the narrow fixture repair;
  corrected runtime replay remains pending.
- The cohort accidentally used serial pytest passthrough because `-v` disables
  the existing batch runner. A new optional `--require-batch-runner` guard now
  rejects pytest flags, absent selectors or disabled workers before collection,
  and refuses an empty collection without a second execution. Intentional
  unguarded passthrough and one-worker batching remain supported. Its regression
  first reproduced the accidental fallback; the corrected workflow suite passes
  **181 tests**, including real process-group cancellation and cleanup
  (`work/logs/required-batch-runner-{red,green}-v1.json`).
- Optional storage-state implementation is isolated in a separate complete
  local CPython checkout and Rust review copies. The selected `1a61fb3c` runtime
  and pinned checker remain unchanged during compatibility repairs. Native
  allocation/GC/freelist and real-checker direct-state acceptance, coordinated
  promotion and the final frozen `just test-all` remain pending. No benchmark
  or remote publication is authorized.
- The lambda crash is fixed by seeding a physical owning entry slot only for
  parameters read by actual raw slot-transfer operations. Other parameters
  retain their borrowed calling convention. The existing prolog and cleanup
  dataflow now share that seed; no opcode schedule or token executor is added.
  The actual lambda plan first failed with an empty owner seed and now proves
  save/normal-restore/error-restore propagation, including rejection of a
  corrupted plan. All **15 class-binding tests** and the five-crate test-target
  check pass. After staging the matching extension, the unchanged authenticated
  crash driver passes (`work/logs/lambda-crash-exact-green-v1.json`), followed by
  **17/17 compatibility cases in five passing batches across two workers**
  (`work/logs/lambda-owner-python-green-v1.json`, 111.61 seconds). This includes
  all four lambda modes/outcomes, all four repaired original validator tails,
  the three original nested-class controls and six eager-frame checks. Native
  source/build/selection postchecks pass. The remaining delimiter-refusal audit
  and final full gate are not replaced by this focused result.
- A six-case delimiter audit reports **12 passed, 12 failed** across ordinary,
  compiled, entry-interpreter and CPython-backed modes
  (`work/logs/delimiter-scope-red-v1.json`). Eight failures merely demand the
  retired missing-local refusal after the original function now executes.
  Removing those four validator overrides restores the unchanged TaskGroup,
  exception-frame cleanup, iterator and import-path assertions; all **16/16**
  original cases then pass in four batches
  (`work/logs/delimiter-original-green-v1.json`, 146.10 seconds). The remaining
  four failures expose a real list-only semantic region decoder/lowerer
  restriction for the original set/dict comprehensions, not a lifetime oracle.
  Their collection-kind and named-expression binding support is under repair
  using the existing SOAC loop/collection operations. No import failure is
  converted into an accepted observer refusal.
- The optional-state promotion adapter has **35/35 passing VM static/unit
  tests**, retaining 32 unchanged transaction helpers and adding guarded joins
  for the separate native and Rust review packets
  (`work/type-state-promotion-v1/static-v2.json`). These use temporary files and
  mocked process/Git calls; no candidate is promoted or built by that gate.
  The current source/build/checker bootstrap suite also passes **170/170**
  (`work/logs/type-state-prepromotion-source-tooling-v1.json`), with unchanged
  selected native source, runtime and build selection. The new optional-state
  native and actual-checker acceptance gates remain pending.
- The eager-region repair now preserves actual list/set/dict kinds and
  separates iteration bindings from outer named-expression writes through the
  new public core `SourceRegionBindingRole`. Cache50 invalidates the previous
  projection. The five affected crate test targets type-check; **2/2** direct
  production-join/scoping tests and **17/17** native class/source-binding tests
  pass. A corruption test originally reached the lowerer's panic wrapper and
  poisoned two later tests; its rejection is now checked at the exact private
  Result-returning production join, with the failed run retained. After the
  matching extension build, all **12/12 original cases across four backends**
  pass, including the unchanged generator-expression assertions
  (`work/logs/eager-collections-originals-v1.json`, 129.18 seconds). Native
  source/build/selection postchecks pass. No source body or original validator
  is replaced, and no new opcode or release-schedule machinery is introduced.
- The next retained counter/cache cohort reports **18 passes, eight failures
  and two fixture-setup errors**, not a clean compatibility gate
  (`work/logs/retained-counter-cache-compat-v1.json`). Actual `ty` rejects the
  old undeclared `Box.x` and `Point.x` source fixtures; their exact originals
  remain ordinary controls with explicit checker-rejection coverage, while
  admitted interoperation passes ordinary instances through unresolved
  parameters. Protocol iteration already records its real strict source ID;
  the old unchecked-function-ID witness was zero. Named generator resumes
  compile eagerly while the public generator factory remains native. Those
  test repairs now pass their combined **10/10** selected-runtime replay
  (`work/logs/retained-counter-repairs-v1.json`, 150.40 seconds including the
  matching extension rebuild, with source/build/selection postchecks passing).
  Unsigned BlockPy caches are
  deliberately bypassed for strict imports; ordinary compiler cache reuse and
  work-directory routing retain structured coverage in the driver test. Its
  test-target check and focused rebuild/reuse test both pass
  (`counter-cache-rust-check-v1.json`, `counter-cache-rust-test-v1.json`).
- The original authenticated width-9 nqueens driver completes with all
  assertions in **98.20 seconds**, unchanged source/artifacts and successful
  native source/build/selection postchecks
  (`work/logs/retained-timeout-repro-v1.json`). Its earlier 60-second fixture
  limit was insufficient; only that expensive strict validation allowance is
  increased to a bounded 180 seconds. The requested stack capture found the
  process had already exited successfully, so it produced no hang diagnosis.
  Both nqueens cases now pass together in **248.04 seconds**. Their original
  ordinary controls remain unchanged; positive tracing also runs under a
  separately authenticated CPython publication. SOAC checks its documented
  explicit observer refusal, unchanged tracer, zero claimed source events,
  cleanup, untraced recovery and sealed mutation rejection. The mixed
  `nqueens-and-field-key-red-v1.json` receipt records those **two passes**
  separately from the new field regression's **three failures** below. This
  is retained behavior validation, not benchmark or optimization evidence.
- Optional-state candidate `5766f93e` failed its first development build because
  `typeobject.c` referenced the dictionary implementation's private
  `CACHED_KEYS` macro. A narrow logical follow-up accesses the actual heap-type
  field and has its own regenerated top. Candidate `bfe76d48` now builds in the
  isolated persistent development directory with verified executable/library,
  mode, exports and source (`work/cpython-type-state/development-v2/complete.json`).
  The failed candidate and log remain retained. Native behavior, actual
  StackRef-debug and Rust integration gates are still pending; the selected
  `1a61fb3c` interpreter and root gitlink have not changed.
- Paired raw-C allocation controls found a genuine optional-state regression:
  ordinary, legacy, custom and cold-direct allocation confused a pre-existing
  exception with a new metadata-preparation error. Baseline controls pass
  **6/6**; candidate `bfe76d48` fails **8/10**, with warm-direct controls still
  passing (`work/cpython-type-state/pending-error-v2/complete.json`). Logical
  `2de425d` saves/restores the original error around preparation and preserves
  genuine new allocation errors. Generated-top candidate `b8dcf1ca` builds and
  passes **24/25** focused storage cases with one declared debug-only skip,
  plus **27/27** legacy field cases. This includes the new pending-error and
  canonical-key controls, actual allocation sizes, GC/shared-rule retention,
  terminal attachments, reentrancy, private escape, native slot rows, alias
  guards, OOM and resurrection. Actual StackRef-debug and broader native gates
  remain pending; the receipts are `development-state-v3/complete.json` and
  `development-legacy-v1/complete.json` under `work/cpython-type-state/`.
- The canonical-key probe also exposed an older shared Rust policy defect:
  `check_value` skipped a stored `str` subclass as though it were non-string
  overflow. A new real-checker test fails in **all three backends** before
  the narrow `PyUnicode_Check` correction. The matching rebuild and **9/9**
  three-backend new/existing field-key tests now pass
  (`work/logs/unicode-field-green-v1.json`, 204.25 seconds, native postchecks
  passing). Subscription, raw C and bulk writes, setdefault insertion/read,
  initial replacement admission, stored-key identity and once-only explicit
  lookup callbacks are covered. Non-Unicode overflow and exact namespace or
  physical-metadata guards are unchanged. The separately frozen Rust review
  v2 preserves this correction in both preimages and optional-state postimages;
  that rebase is not yet native integration evidence.
- The unchanged candidate's development replay also passes **245/245** retained
  native/C/C++ selections, **211/211** additional type, slot, dataclass,
  descriptor and dictionary-transition selections, and the three ordinary
  CPython modules `test_dict`, `test_gc` and `test_weakref`. Actual
  StackRef-debug now passes **25/25** new storage cases without a skip,
  **27/27** legacy field cases and **245/245** retained selections, including
  the genuine marked-allocation zero-refcount diagnostic. These phase counts
  are not a claim of distinct tests across overlapping suites. Receipts under
  `work/cpython-type-state/` preserve source/fixture/runtime and unchanged root
  selection postchecks; remaining debug compatibility and root integration
  are still pending.
- The selected pre-promotion runtime passes **597/597** pure Rust tests:
  49 core, 16 driver and 532 lowering
  (`work/logs/semantic-core-lowering-driver-v1.json`, 41.63 seconds). Five old
  synthetic-function/late-owner Python tests separately reproduce fixture
  failures: their unmarked modules run ordinarily, so synthetic callbacks or
  `profile.bin` are absent. The two receipts are
  `legacy-synthetic-compat-red-v1.json` and `legacy-late-owner-compat-red-v1.json`
  under `work/logs/`, with native postchecks passing. Their migration preserves
  original ordinary controls and adds authenticated semantic counterparts;
  unsupported positive optimization expectations are not admission authority.
  Actual checker/runtime replay of those repairs remains pending.

- The completed optional-state native matrix passes **507 selections with one
  declared debug-only skip** in development, and **508/508 selections without
  skips** in the actual StackRef-debug build. The groups can overlap and are
  not distinct-test totals. Ordinary `test_dict`, `test_gc` and `test_weakref`
  pass **306 cases plus six declared skips** in development and **307 plus five
  skips** in debug. Source, fixture, executable/library, allocation-mode and
  unchanged root-selection checks pass. The frozen evidence is
  `work/cpython-type-state/validation-v1.json`; the supported native layout is
  64-bit little-endian with the GIL, tested on Linux AArch64. Neither this
  isolated evidence nor the source promotion below establishes the real-checker
  Rust/native join.
- The migrated synthetic/closure/eager cohort reports **eight passed, four
  failed** (`work/logs/legacy-synthetic-compat-green-v1.json`; the stem is not
  its outcome). Actual returned closures, shared source-code metadata and
  explicit observers pass. Three eager failures expose the single-region
  decoder/lowerer restriction in the unchanged canonical source. The fourth
  was a bad global oracle: an actual lexical `global generation` declaration
  intentionally authorizes rebinding. The signed artifact and isolated native
  replay confirm this while still rejecting replacement of the frozen `events`
  binding (`work/logs/eager-global-policy-v1.json`). The corrected fixture uses
  the exact original source, with no mutable-holder rewrite. Its replay and the
  sibling/nested-region repair are pending. A structured tests-first run fails
  at the native semantic join (`work/logs/eager-multi-region-join-red-v1.json`);
  the repair carries lexical parents, shared actual-slot carriers and distinct
  per-emission snapshots, with cache format 51. These are scoping and safe
  ownership requirements, not opcode-lifetime matching.
- The two migrated late-owner fixtures also fail their first authenticated
  replay (`work/logs/legacy-late-owner-compat-first-v1.json`). Nonself behavior
  and checks pass, but its old counter oracle excludes valid indexed-fallback
  reads. Read-only decoding confirms the per-field generic-plus-fallback counts
  and zero indexed hits (`work/logs/late-owner-profile-decode-v1.json`). Scalar
  profiling succeeds, then verify import exposes a genuinely unattached
  ineligible branch plan after linearization. The tests retain source semantics,
  original ordinary controls and actual source IDs. A bounded production repair
  is prepared but unapplied: only missing attachments lacking the already
  required field guards may decline; valid missing nodes still fail and present
  plans retain later virtualization and final validation. Runtime red/green is
  pending; no new optimization eligibility is requested.
- A separate retained dispatch/storage cohort passes **12 cases** and fails
  two before execution on a checker Jujutsu pin query
  (`work/logs/retained-dispatch-storage-compat-v1.json`). The original subprocess
  diagnostic was discarded and a later read-only query succeeds, so its cause
  remains unknown. The verifier now preserves bounded escaped command/return
  code/output evidence, the checker labels its phase, and fixtures keep unique
  combined logs. No fallback or admission bypass was added. Focused diagnostics
  pass **11/11**, followed by **175/175 `just test-source-tooling` tests**
  (`work/logs/source-tooling.e8bW3A.log`,
  `work/logs/jj-pin-source-tooling-full-v1.json`). These are tooling gates, not
  the missing two runtime results or the full project gate.
- Coordinated local promotion completed **2026-08-25 03:26:01 PDT**. The actual
  JJ tree pins CPython `b8dcf1ca1a138253c51c8733e52e597d7db68abf`, its separate
  generated top above logical `2de425d162a456062b80d0119fbc0e874d208a64`.
  Seven reviewed integration/fixture postimages were published, with Ruff,
  unrelated root bytes and the user-owned runtime-type-state document preserved.
  Both native source and checker remain on the shared mount; complete local
  object stores and official native regeneration are verified. The promotion
  receipt is `work/type-state-promotion-v2/manifest-v2.promotion/complete.json`.
  The old saved build selection remains unchanged and deliberately stale.
  A fresh persistent optimized `--no-select` build started at
  **2026-08-25 03:30:56 PDT**, with actual host and guest free-space checks and
  frozen native/build-tool inputs. Its terminal proof, selection, matching
  extension, integration matrix and full gate are not yet claimed.
- The fresh optimized build completed at **2026-08-25 03:41:37 PDT**, with all
  frozen-source, tool-input, executable/library, nondebug GIL PGO/LTO and
  unchanged-selection checks passing
  (`work/type-state-promotion-v2/build-v1/terminal.json`). It is now selected at
  `/home/adamh.guest/.local/share/soac/builds/type-state-optimized-v1-01a02587`;
  a separately verified manifest is retained at
  `work/type-state-promotion-v2/expected-runtime-v1.json`. The first six-crate
  test-target check caught test-only PyO3 imports, integer inference and an
  unnecessary serialization assertion referring to an absent crate dependency.
  Those are corrected without a production change or dependency addition.
  Matching extension, structured red/green and full-gate results are pending.
- After the test-only corrections, all six affected crate test targets
  type-check (`work/logs/type-state-rust-check-v2.json`). The matching extension
  and venv build also pass (`work/logs/type-state-extension-v1.json`, 102.61
  seconds), with the new source/build/selection identity unchanged. Independent
  Rust review found no additional actual-owner, projection-retention or
  capability issue. The original-Name-to-native-slot corruption regression and
  stale scalar-plan regression are scheduled before their production guards;
  full runtime acceptance is still pending.
- The wrong-slot source regression and unattached scalar regression each fail
  at their intended assertions before the reviewed repairs
  (`work/logs/eager-name-join-red-v1.json`,
  `work/logs/scalar-late-attachment-red-v1.json`). Both native postchecks pass.
  The repaired six-crate test-target check and matching fast extension rebuild
  pass. The combined structured run then reports **21 passed, five failed**
  (`work/logs/semantic-joins-green-v1.json`): all five remapped-plan checks and
  normalized/private-name controls pass; one nested-region test incorrectly
  assumes every native region saves exactly one local, and the other four
  failures are shared-test-lock poisoning. A compile-only native metadata probe
  confirms that the exact canonical outer dictionary records both its own
  `value` target and an `inner` target belonging to its child region
  (`work/logs/eager-native-saves-probe-v1.json`). The semantic projector must
  isolate each region's own target without recreating native inlining saves;
  that follow-up and the complete class-binding green run remain pending.
- The promoted optional-state integration passes all **seven** focused JIT
  projection/ownership/reference tests and all **11** standalone raw-runtime
  tests (`work/logs/type-state-rust-gates-v1.json`,
  `work/logs/type-state-raw-runtime-v1.json`). These execute actual stateful
  allocations for the JIT marker test, not synthetic flags on ordinary objects.
  Real-checker storage and final compatibility gates are still running or
  pending; these focused passes are not the full-gate result.
- The final observation audit preserves the original four native frame-cleanup
  validators while giving SOAC explicit event/value/exception assertions and
  exact-once quiescent cleanup. The method-call control retains its source and
  call exercise, checks explicit argument order, and now includes dispatcher
  teardown in eventual cleanup instead of comparing destructor micro-order.
  A separate deletion companion checks immediate binding removal with a live
  alias. The `assert_raises_refcount` delimiter keeps every source byte and the
  stock in-body count equality; retained backends check exception chains and
  warmed caller-side ownership balance after return. No semantic failure is
  made an expected failure. These reviewed test splits still require runtime
  replay. A stale direct-call documentation claim was also corrected against
  the actual public-boundary admission, zero unchecked target ID, guarded
  retained call path and native final-global policy; no optimizer change was
  made for that wording correction.
- The optimized real-checker storage matrix selected 71 nodes in **18 passing
  batches and one failed batch**
  (`work/logs/type-state-python-matrix-v1.json`). All 18 new storage scenarios
  pass across compiled SOAC, entry-interpreter and JIT-disabled CPython:
  fresh direct state and ordinary copies, legacy replacement/custom allocation,
  distinct actual nominal targets, escaped dictionary retention and final
  dataclass storage projection. The native fixture file also passes its batches
  with its declared debug-only skip. The single failure is the retained
  field-read oracle expecting two indexed-capability publication events, not a
  failed field operation. Its recorded apply run emits all four expected
  codegen records and zero indexed grants. The reviewed test-only correction
  uses actual distinct native class/function owners and checked public entries,
  asserts ordinary dictionaries and no indexed grants, and preserves every
  original source/value/callback/mutation and verify-counter check. The
  original failed run never reached verify; its counters are not claimed.
- The nested-save follow-up keeps the complete native inventory but selects
  only each region's direct source iteration binding. A native extra save must
  correspond to an active descendant's iteration binding; it does not create
  another SOAC snapshot or reproduce a CPython instruction schedule. Focused
  tests include reordered raw inventory, unrelated-extra-save rejection, and
  original-source nested scoping/error/recovery/eventual cleanup. The six-crate
  test-target check and matching extension rebuild pass
  (`work/logs/eager-inventory-check-v1.json`,
  `work/logs/eager-inventory-extension-v1.json`). The isolated structured and
  behavioral reruns remain pending. The isolated sibling/nested regression now
  passes, followed by **27/27** class-binding and remapped-plan checks
  (`work/logs/eager-inventory-isolated-v1.json`,
  `work/logs/eager-inventory-structured-v1.json`). Independent review identified
  an additional source-binding completeness gap: checking supplied Name/access
  rows does not reject an omitted element Load. A tests-first corruption case
  is prepared; it has not yet been executed or repaired. This concerns actual
  name resolution, not native execution-schedule matching.
- The 40-node semantic matrix completed with **16 passing batches and one
  failed batch**, with unchanged native source/build/selection
  (`work/logs/semantic-compatibility-matrix-v1.json`). Nested comprehension
  normal/error recovery, eager observers, actual closure/code metadata, all
  twelve original frame-cleanup cases, the three refcount-delimiter controls,
  method-call cleanup, both late-owner tests and the corrected ordinary field
  read all pass. The field-read run now reaches and passes its original verify
  counter assertion. Both previously interrupted source-preflight cases also
  pass, without treating that replay as an explanation of the earlier JJ
  failure. The two new deletion-companion nodes fail before runtime: `ty`
  correctly rejects their deliberate post-delete unresolved-name read.
  The original source and validator are retained unchanged as an ordinary
  control and explicit strict-admission rejection; an independent checker-valid
  companion uses supported exception traceback locals to test immediate
  binding deletion, live alias identity, callback errors, recovery and eventual
  exactly-once release. It does not introduce general function `locals()`
  support or suppress diagnostics. That companion's replay remains pending.
- The omitted-element-Load corruption regression fails at its intended
  assertion (`work/logs/eager-access-completeness-red-v1.json`), after its
  unchanged positive source lowers successfully. The reviewed repair joins
  source occurrences and native access rows in both directions using the
  original ordinary-local catalogue, active iteration bindings and actual
  function/lambda ownership. Global names do not become local merely because
  a sibling region allocates a same-named native slot; initial iterables and
  lambda defaults retain their source owner. Seven positive scope-boundary
  cases accompany the repair. No wire schema, public API or runtime execution
  metadata is added. Formatting passes; compilation, matching extension and
  behavioral replay are pending.

### Full gate and compatibility repair

The final-`023ac` optimized `just test-all` attempt finished with status 1 at
**2026-08-24 11:34:26 PDT**. Its exact 1,045-file input manifest remained
unchanged. Workspace Rust and standalone raw-runtime stages passed. The pytest
runner reported **813 passing batches, 117 failed batches and two timed-out
batches**, not 813 passing test cases. The terminal inventory includes **211
distinct reported nonpassing node IDs** (209 failures and two errors), including
three failures printed before a timed-out batch could write its summary.
Receipts: `work/logs/enforced-final-023ac-test-all.json` and
`work/enforced-final-023ac-failure-inventory-terminal-v2/ready.json`.

The following repair pass is not a replacement full-gate result:

- Actual tests-first failures demonstrated named-expression target readback,
  missing generator-argument source ranges, premature class-cell name-binding
  assertions, operand retirement order, and secondary traceback-rendering
  failures. The corresponding production changes and focused structured tests
  are applied. Matching extension builds and cross-crate test-target checks
  pass; the first repaired lowerer run passed 526 tests and exposed two stale
  named-expression shape assertions, subsequently corrected. The complete
  corrected rerun passes **528/528** (`enforced-postgate-v3-green-lowering`).
- The next Python selection passed **84 of 88 tests**. All 20 assignment-
  operand lifetime cases, named-expression behavior, and primary-error reporting
  passed. The two new real-collection failures were recorder indentation bugs;
  their corrected rerun passes. Ten of twelve retained suspended-operand cases
  passed, but the two successful-resume cases still observed an extra argument
  reference. All six corresponding ordinary controls pass. The new consuming-
  call helper has **five passing structured/cleanup tests**; that intermediate
  result does not establish actual suspended-call correctness.
- A subsequent structured test follows the original suspended call through the
  actual runtime `Some(profile)` planner. It fails after a fresh final-argument
  call becomes `Load`, although the no-profile baseline preserves consuming
  inputs. The shared IR predicate and atomic-call linearization repair are
  applied. Its six outgoing-call structured and cleanup tests now pass
  (`enforced-outgoing-profile-plan-green-v1`), including the actual runtime
  planner regression; this is separate from the Python rerun below.
  The seven changed crates' test targets type-check. A static review caught
  two test imports lost while moving the predicate; their explicit test-only
  supplement was included before that check.
- Corrected reporting exposed a duplicate augmented-assignment operand and a
  second mandatory retirement across `await`. A new **18-case** original/native
  comparison records **14 failures and four passes** before the repair: six
  completion paths raise `UnboundLocalError`, and eight attribute/subscript
  failure/close paths expose the compiler-only caught exception to finalizers.
  Separate structured tests reproduce the missing move and erroneous handler
  entry. The consuming old-value handoff and five `Preserve` delegation blocks
  are applied, with BlockPy cache version 45 rejecting older bodies. A lowerer
  rerun passes 529 tests and finds one overbroad new assertion that also selected
  terminal teardown; that assertion now selects only delegation transport and
  preserves the distinct source-event dispositions. The corrected complete
  lowerer rerun passes **530/530** (`enforced-postgate-v4-lowering-green-v2`).
  The matching-extension Python rerun passes **43/43** in **243.44 seconds**:
  six ordinary suspended-operand controls, twelve retained suspended-operand
  cases, all eighteen new augmented-await cases, and seven import/refusal/native
  controls (`enforced-postgate-v4-operand-imports`). No ownership or finalizer
  observer was relaxed. The separately named closure-backed delimiter case was
  not collected by this selection; its later ordinary/compiled/entry-interpreter
  rerun passes all three cases.
  The authenticated joined-loop profile/apply test then reproduced
  CLIF verification failure from a nondominating value. A traceback formatter's
  request for unavailable `tb_lasti` is instead the documented explicit
  activation-introspection refusal; its retained refusal and unchanged native
  positive are now separately enrolled, not converted to an xfail.
- The list-store failure is now isolated in an actual-source structured test:
  a consuming RHS clears compiler ownership separately in two scalar-index
  guard arms, leaving a branch-local value in native source error projection.
  Two artificial IR probes passed because they did not carry that projection;
  a first actual-source attempt failed setup because runtime module IDs were
  mistaken for serialized table indices. Those outcomes remain distinct from
  the corrected actual-source **RED** (`enforced-scalar-setitem-source-red-v4`).
  A per-operation replayability check now selects the existing once-evaluated
  boxed-index path for consuming or effectful replacements, retaining the same
  exact-list plan and counter identities. All **seven** focused store/ownership
  tests pass (`enforced-setitem-ownership-green-v5`), including that regression,
  borrowed/owned replacement controls, and the structured selection matrix.
  Matching extension and test-target checks pass. The unchanged original
  joined-loop profile/apply case now passes. The corrected broader fixture
  cohort passes **9/11 cases** (`enforced-postgate-authenticated-profile-fixtures-v2`):
  the two remaining failures are fixed-unpack intrinsic identity and an obsolete
  unchecked method-hit assertion, not a recurrence of the list-store failure.
- The original fixed-unpacking behavior case observes two calls to a replaced
  Python helper in apply mode. A new actual-source structured test reproduces
  loss of the `UnpackFixed` intrinsic in the real runtime-profile planner
  (`enforced-fixed-unpack-intrinsic-plan-red-v2`). An earlier test invocation
  failed to compile because of a missing test-only type qualification and is
  not counted as a behavioral failure. The narrow repair keeps compiler-owned
  language intrinsics explicit through binding and typed expression
  linearization; ordinary callable reads still preserve evaluation order.
  All four focused JIT tests now pass, including the actual-source planner
  regression (`enforced-fixed-unpack-intrinsic-plan-green`); all seven existing
  ordered-linearization/ownership controls pass
  (`enforced-fixed-unpack-linearization-controls`). Cross-crate test targets and
  a matching extension build pass. The unchanged original Python unpacking case
  passes profile and apply in **42.21 seconds** in the broader rerun below.
- Legacy hook-only fixtures are being moved to actual checker/startup admission
  with ordinary controls and native owner/seal witnesses. Sealed mutation
  differences are explicit. Optional indexed layouts and legacy generator
  wrappers are not fabricated to rescue obsolete optimization assumptions.
  The first broader migration run exposed incorrect new witnesses: the legacy
  `PyFunction_GetSoacFunctionId` slot authorizes unchecked targets and deliberately
  remains zero for mandatory checked entries. Corrected witnesses require the
  actual native owner, strict seal and checked entry instead; production does
  not publish an unchecked target to satisfy a test. Class enforcement and
  constructor fast-path eligibility likewise remain separate. The corrected
  fixture cohort passes the nine cases reported above. A separate six-case
  owner/dispatch cohort reaches one strict-rejection pass and five failures:
  two unsupported-source checker diagnostics, two new test-witness API/import
  mistakes, and obsolete import-time unchecked-target assumptions. Those
  failures are being repaired without adding optional runtime capabilities.
- The next eight-batch compatibility run passes **10/14 cases**
  (`enforced-unpack-owner-dispatch-compatibility-v2`): original fixed unpacking,
  both resolved-method cases, all four profiled function-mutation controls,
  import-time constructor behavior/activity, and two explicit checker
  rejections pass. Three field fixtures still incorrectly require permanent
  seals on statically dynamic class methods; actual owned checked entries do
  not imply that seal. The immediate-method original comprehension also exposes
  a real missing source-frame projection for native local `value`. That remains
  a compatibility failure, not a test-only correction or expected rejection.
- The isolated current-checker baseline is independently verified: **1,094
  passing tests and 34 existing upstream ignores**, composed from four successful
  upstream commands and a new 36/36 native-backed wrapper run. The earlier
  wrapper's 24 missing-environment setup failures remain recorded as failures.
  The new ordered class-tail checker change remains isolated. Corrected
  tests-first execution reaches four intended feature failures with no setup
  or control errors. Exact production commit `f96bd342` and separate reviewed
  formatting commit `1343c80b` are independently verified against the preserved
  final canonical tree. The formatter's initially rejected extra baseline file
  remains a failed attempt, not a restamped success. Candidate focused after
  tests pass **4/4** and the broader upstream commands pass **63 core, 361
  semantic, 152 project and 485 Markdown tests**, retaining 34 existing upstream
  ignores. The actual verified native-backed wrapper passes **36/36**. The
  complete candidate after gate is **1,097 broad-suite passes, zero failures
  and 34 existing ignores**, plus four overlapping focused reruns. Independent
  ROOT verification rehashes logs, executables, source, native provenance and
  resource evidence (`work/checker-static-attributes-after-root-verification.json`).
  Logical-history/lockfile finalization also passes: local final head
  `e8a81e2f2f021b9fd34a003acd121bd6c6590130` retains the successful candidate
  tree, with the regenerated lockfile in its separate top commit
  (`work/checker-static-attributes-local-validation-v2/execution-v2/finalize.json`).
  Independent fresh-checkout replay now also passes **1,097 broad-suite tests**
  and the four overlapping focused cases, retaining 34 existing ignores. The
  final replay rehashes all 41 command log pairs, six actual executables, source
  identities and frozen harness inputs (`replay.json`, SHA-256
  `5a1b4ea25e11fb7790b462edf0f5e91759a811b9886c29dc121600c5b90fe2cf`).
  The 4-GiB resource ceiling produced memory-pressure events but no OOM or
  OOM-kill. Selected-generation promotion remains a separate gate. No new
  checker generation has been promoted.
- The corrected dynamic-class method witnesses clear all three earlier seal
  assertions (`enforced-owner-field-seal-witnesses-v3`). That run passes the
  explicit whole-source rejection and exposes three remaining failures: the
  uniform case's native comprehension local `owner` is missing its owning-frame
  projection, and inherited/late-bound cases have empty type-key profiles.
  Both field cases already pass their original profile-mode behavior and
  positive generic field counters; they stop before verify/apply. Source
  inspection identifies a missing split-key observer in compiler-owned strict
  class construction. Two new actual-admission tests reproduce that absence
  in compiled and entry-interpreter modes
  (`enforced-class-key-observer-red-v1`). The observer repair, Rust test-target
  check and matching extension build are complete. The final cohort now passes
  **4/4 cases in 108.21 seconds** (`enforced-field-compatibility-checked-v1`,
  log SHA-256 `d817f95c53e3479b95c1503ae541038a8a119140af16db67a087598a30b68fcc`).
  The original field bodies, validators, type-key observations and positive
  native-slot witnesses remain intact. Counter expectations now distinguish
  installed ordinary slots (Point retains positive indexed hits) from ordinary
  dictionaries with no installed optional indexed-layout capability (Record
  and inherited dictionary accesses require zero indexed hits and positive
  generic/fallback counts). Profile, verify and apply all reach the original
  behavioral assertions; the dated scope does not require installing a new
  indexed dictionary optimization to satisfy those controls.
- Native-reference readiness now lives in the original source projection and
  native code catalogue, with BlockPy cache version 46. It joins native
  parameter order, physical source locals, exact read/pair lanes, binding and
  CALL emissions, and captured-code constants to resolved instruction IDs.
  Four structured tests pass (`enforced-source-reference-readiness-tests-v1`),
  including all six unchanged ownership subjects and malformed/stale/ambiguous
  correspondence rejection. Cross-crate test targets and the matching
  extension also pass. These are compiler-kernel correspondence tests, not
  checker/startup admission or runtime ownership proof. SourceEntry registration
  and both actual token-body consumers remain incomplete.
- The synchronous-body native A/B candidate is locally committed at
  `3000af3e9598ace5d77d90b958b7acfae629fedc`, above logical change
  `6640a83e53bea9ccb6a8a07c4b04bc3e5eca785e`. Actual regeneration matches the
  selected `023ac` generated bytes. Its fresh development `--no-select` build
  passes, followed by **707 passing native cases and one existing debug-only
  skip**, each of the 708 IDs in a distinct subprocess. All ten new interval,
  outgoing-call and constant-borrow cases pass; no failures or xfails occur.
  Receipts are under `work/cpython-sync-body-joins-commits/development-v2/`.
  The fresh StackRef-debug `--no-select` build then passes **708/708**, with
  zero skips, failures or xfails, including the actual debug-only misuse test.
  Its terminal summary hashes to
  `71d872d7253e98dcd2a9600bf2b2e4f22f636db7a21646b9a43695998c7f5ade`
  under `stackref-debug-v1/gate/`. All case logs, actual runtime/provenance,
  source and fixture hashes were rechecked; the scope is stopped. Source
  promotion, optimized build and the complete Rust token consumer remain
  pending. The live native pin and saved build selection are unchanged.
- The original-parent eager-comprehension repair now retains the current and
  saved native local carriers through both normal and exceptional cleanup.
  It handles the original fused `STORE_FAST_LOAD_FAST` lanes and validates the
  final actual source-code join. BlockPy cache version 47 covers these source
  regions and the original instruction offsets added to reference recipes.
  The structured regional tests pass, as do the four existing source-reference
  tests and the HIDDEN-local rejection control. The actual runtime cohort passes
  **6/6 cases** (`comprehension-region-runtime-v1`), including ordinary controls
  and strict compiled/entry normal/error paths. The unchanged immediate-method
  source and behavioral validator also pass profile, verify and apply
  (`comprehension-region-immediate-v2`). Its only updated structured expectation
  names the actual original parent instead of a nonexistent outlined helper.
  The uniform-field case now reaches a separate forbidden base-adoption
  attempt; the native rejection is consistent with the existing strict
  inheritance contract, and its strict validator is being corrected while
  preserving the complete ordinary exercise.
- The native source-reference consumer is now implemented in both retained
  execution modes. The compiled entry has a distinct opaque-token ABI; the
  entry interpreter walks the original BlockPy with native token storage.
  Both use the actual native binding, one original source frame, mandatory
  borrowed parameter/return checks, and ordered local/support teardown. The
  original ownership fixtures now require actual SourceEntry registration,
  and a new two-argument finalizer control checks reverse native local order.
  The six affected crates and all their test targets compile successfully
  (`enforced-native-consumer-check-v1`, **29.29 seconds**, log SHA-256
  `f078f468f271b6faf1666534fd320803112c67892424f9a37a0984de2170a619`).
  This is a compile check, not runtime ownership evidence. The matching native
  source promotion, optimized rebuild, extension staging and behavioral runs
  remain required before those stronger assertions can be counted as passing.
- Coordinated local promotion completed **2026-08-24 21:01:08 PDT**. The actual
  JJ tree records CPython `3000af3e9598ace5d77d90b958b7acfae629fedc` and Ruff
  `e8a81e2f2f021b9fd34a003acd121bd6c6590130`; both child HEADs and canonical
  source bytes agree. All 22 native fixture postimages, unrelated root files,
  working change identity and parent were preserved; the temporary pin carrier
  was removed. No network publication occurred. Evidence is
  `work/coordinated-source-promotion-v3/promotion/complete.json`.
  The first promotion attempt stopped before mutation because a same-UID
  nondumpable `sd-pam` process denied unprivileged `/proc` inspection. The
  retained v3 repair uses narrow read-only privileged reads, checks PID/UID
  identity, and never exempts an unreadable process. Eight focused tests and
  the actual full process/scope scan pass. The saved build selection remains
  byte-identical and deliberately stale; a fresh optimized build and matching
  extension are required before any runtime consumer resumes.
- The source-tooling bootstrap gate now passes **172/172 tests in 8.00
  seconds** while that native build is in progress. The first run passed 170
  and exposed two case-sensitivity fixture setups that attempted to persist an
  external `/tmp` build. Moving only their previous valid build fixture beneath
  the fixture checkout preserves the real persistence guard and all ordering
  assertions. Logs are `work/logs/source-tooling.2Qde2s.log` (before) and
  `work/logs/source-tooling.aY4YH2.log` (after, SHA-256
  `ff1d707406f804e34e401991529b21fcd7b0e10fe6a73cd6cdba93addd677945`).
  This system-Python tooling gate does not execute or validate the new native
  runtime, and does not replace `just test-all`.
- The promoted `3000af3e` source completed a fresh actual-ROOT optimized
  `--no-select` build at **2026-08-24 21:16:39 PDT**. The normal build's PGO
  training succeeded across 43 modules; final nondebug PGO/LTO executable and
  loaded-library identity passed verification. Peak scoped memory was
  1,315,713,024 bytes, with zero max/OOM/OOM-kill or swap events. Source,
  tooling and the old saved selection were unchanged through the build.
  Receipt: `work/coordinated-enforcement-validation/build-v1/terminal.json`;
  provenance SHA-256
  `416951be1882a432153fc931b51872ab54bfaf91c04b2c067e396d464c1202f8`.
  A separate verified selection then selected that build. The frozen runtime
  manifest is `work/coordinated-enforcement-validation/expected-runtime.json`,
  SHA-256 `12b60e2d57f7bf4c00a7e1fc3243871f14bf9b820e11d6c4e5758bc1d5f6a8d0`.
  The six-module ordinary CPython sample also passes (851 tests, two skips and
  two expected failures), without a SOAC import hook, against that exact
  manifest (`work/logs/ordinary-cpython-six-v1.json`). Neither build training
  nor this ordinary sample replaces the strict ownership and full-gate runs.

Current native and checker pins remain local commits. No pushes, optimization
benchmarks, or new performance claims were made. The execution-compatibility
correction above replaces the proposed binding-token continuation work. Full
semantic/safety compatibility and a new optimized `just test-all` remain
required after the matching-only machinery is removed or simplified.

### Native enrollment checkpoint

The dated amendments in `OPT_GOAL.md` and `doc/TYPE_DRIVEN_OPTIMIZATION.md`
replace the optimization roadmap with the authenticated `ty` -> actual runtime
type/function binding -> interpreter enforcement milestone. The acceptance
path must execute ordinary CPython frames with SOAC JIT execution disabled.
The existing SOAC entry interpreter is a lowered execution path, not evidence
of that acceptance boundary. Native strict frames currently refuse unowned
execution; removing that refusal without actual authenticated execution owners
would be a contract violation, not an interpreter implementation.

The August 24 amendment adds native pending-type protection before callbacks:
block allocation and `__class__` reassignment into pending types (including
supported C and inherited paths), then bind and install constraints on the
actual final decorated type before enabling instances. Fresh replacements need
their own linked guard. An unselected provisional type may become dynamic only
if no permanent type contract was installed; existing contracts remain intact.
The promoted native pin is now `023acfa7a20df9d4ac74afbac542587e766339a9`.
Its fresh actual-shared-source optimized build has completed. Independent root
verification matches all 5,565 canonical source files, four tooling inputs,
22 fixture postimages, the build log, and eight new/previous runtime and
provenance files. A fresh actual-process observation confirms the nondebug
PGO/LTO executable and loaded library. The verification receipt is
`work/source-history-migration-integration/final-023ac-optimized-build-root-verification.json`.
The complete native gate on this final build is **697 pass and one exact
StackRef-debug-only skip**, with all 698 distinct case processes exiting zero.
Independent root verification rehashes all 1,419 artifacts, three binaries and
1,396 child logs, checks actual discovery and outcomes, and confirms all 22
live fixture postimages are unchanged. The ready receipt is
`work/cpython-inherited-dictionary-catalogue-commits/actual-root-optimized-v1/ready.json`
(`ed58f70c5a9d9560efa6150102303949723f3cf8531aab0a43dbaff46262aee5`);
root verification is
`work/source-history-migration-integration/native-023acfa7a20d-optimized-root-verification.json`.
Normal `just select-cpython-build` now selects the matching final optimized
build. The prior `05e18` runtime and receipts remain preserved as historical
evidence; they are not reused with the new source.
On the prior matched `05e18` build, the Rust join and workspace test targets
type-check. Five actual checker/startup-configured interpreter admission tests
pass, as do the three native Pydantic/Django/SQLAlchemy compatibility controls.
The unchanged seven saved dataclass cases now all pass; together with four new
failed-Apply cleanup cases and three explicit-descriptor callback cases, the
matched gate is **14/14 pass**. Ten retained-SOAC Pending-type cases also pass
through actual native required-boundary witnesses. Broader compatibility and
the final full gate remain incomplete; no end-to-end completion is claimed.

The additional mode-0 construction barrier passed **413/413** focused tests in
both development and optimized builds. An inventory comparison then found four
older native families omitted from that focused selection. Their additional
283 cases produced 281 passes, one error and one genuine StackRef-debug-only
skip: the combined inventory is **695 unique cases, 693 pass, one error, one
debug-only skip**. The error is an unintended descriptor-code-identity exception
category change. The existing negative fixture remains unchanged; a separate
native logical correction routes identity rejection through the original
callback-free mutation check, without dereferencing an unmatched expected code
pointer. Its regenerated local candidate
`e0ba7e50f379d7df1906a7bb1521b9eccebe3015` passes the complete optimized
inventory: **694 pass, one exact debug-only skip**. Independent root verification
rehashes all 34 receipt artifacts, three binaries and 1,390 child logs. The
matched StackRef-debug run executes every case, including the former skip, but
finishes **692 pass, three shutdown aborts**. The three bodies report success
before `_PyFunction_ClearCodeByVersion` asserts at process shutdown; body success
is not a passing process. CREATE-time code replacement caused MAKE_FUNCTION to
associate the original version with a different current code object. A narrow
identity-guard correction is committed separately at logical
`27810c149857f56f24f34fa16f53a8e05853779c`, with regenerated-only tip
`05e18c98243710bc9ff1d2f33d3ceef2af341aa7`. It compares the actual function code
before releasing the original code StackRef and initializes a version only
when those identities match. A replaced function remains conservatively
unversioned; no early version publication or new ABI is required. Its fresh
development build passes build/import/provenance checks and the complete
**694 pass, one exact debug-only skip** process gate. Independent root review
rehashes all 40 receipts, three binaries and 1,390 child logs. In the fresh
StackRef-debug build all **695 cases pass**, including the three original
shutdown failures and the debug-only misuse control. Independent root review
rehashes 48 artifacts, three binaries and every child log. The corrected commit
was promoted locally after all old native consumers stopped; the exact shared
source and 22 fixture postimages were verified. A fresh optimized build from
the actual shared `vendor/cpython` source passes normal source/build/import
checks and an actual nondebug PGO/LTO executable/library proof. Its complete
native gate passes **694 cases with one exact debug-only skip**, all 695 child
processes exiting zero. Root independently verifies all 1,413 retained artifacts,
three binaries, exact discovery, all child outcomes and the unchanged 22 fixture
postimages. Normal `just select-cpython-build` selected that then-matching shared-
source build, and ordinary `just cpython-info` reported verified optimized
provenance with no source override and the same repository/vendor VirtioFS
mount. The matching extension builds and stages successfully through ordinary
`just build-test-runtime`; its full gate was still pending. The rejected
descriptor-only candidate and its shutdown evidence remain preserved.

Fourteen ordinary CPython regression modules also complete with no unexpected
failures on the matched corrected development build: **1,269 pass, eight
existing skips and two existing expected failures**. They cover types, classes,
descriptors, function attributes, generators/coroutines/async generators,
dataclasses, dictionaries, weakrefs, GC and function/type/watcher C APIs. Actual
discovery exactly matches all 1,279 JUnit identities. Each fresh isolated process
proves absent SOAC ownership/import hooks before and after execution. Root
independently rehashes all 74 artifacts and verifies process outcomes, ordinary
native state and actual executable/library identities. This is a selected
ordinary subset, not a claim that the entire CPython suite was run.

The latest helper/retained/field gate is **27 pass, three fail**. All three
failures are the same incorrect fixture witness: an explicitly opted-out base
can be permanently class-sealed without an empty dictionary field policy. Its
validator now requires the actual seal and absent field policy, preserving all
checked-child and inherited-field assertions and adding Python/C opt-out write
controls. The native class/method/nominal-field/dataclass-decline enrollment and
that correction then complete **27 pass, four fail** across 31 cases. All
nominal/`Self` field, final-method, C method-lookup, unknown-dataclass and corrected
inheritance cases pass. Three failures are the old blanket instance-dictionary
replacement rejection; the other is a missing outer test import before a driver
is created. All four corrected cases pass in the focused rerun. These are enforcement
checks, not new dispatch or storage optimizations.

The workspace Rust preflight reaches **714 pass, 183 fail** in `soac_jit`.
Exactly one failure is primary: its native recursion-layout probe reads the
soft-limit offset as 120 rather than the actual 128. The other 182 failures are
explicit shared-test-lock poisoning, not independent runtime regressions. The
existing raw mirror omitted `_PyThreadStateImpl`'s observer reservation word.
Adding that one field makes the unchanged actual ABI/structured-load regression
pass; the complete workspace is being rerun with an unpoisoned process. This is
retained-path correctness, not a new optimization.

That rerun reaches **768 pass, 129 fail** in `soac_jit`: two primary failures and
127 lock-poison reports. One method-capability fixture still expected an indexed
dictionary despite selecting no field policy. Correcting that single oracle to
the actual absent dictionary policy makes the original method/adoption/checked-
entry/default/alias test pass. The other test requests a retained JIT lifetime
plan for class-body comprehension regions that the native projection deliberately
does not represent. It fails before lowering, not during Python execution. Its
original source and existing ordinary lifetime observer are being migrated to
the actual interpreter acceptance path; the projection must remain fail-closed.

The expanded native boundary gate is **12 pass, three fail**. C module-dictionary
atomicity, function metadata setters, compatible vectorcall replacement, component
policy opt-outs, whole-union dynamic fallback and generator-close controls pass.
All three failures reach the same old malformed public resume-API probe after
the existing closure, coroutine and async-generator behavior assertions pass.
The retained ABI rejects the invalid state capsule; the CPython backend rejects
missing JIT metadata. These are not synchronous-binder bypasses. The fixture
now distinguishes malformed-state rejection from wrong-role rejection using an
actual preserved-state object, without changing the runtime's precondition order
or accepting arbitrary exception categories. All three modes pass afterward,
including the original body/closure checks and new native ordinary-binder,
object-kind, no-eager-body and no-JIT witnesses. The existing `test-all` workflow
test also passes with `--no-fail-fast`: Cargo now attempts later test targets
after an earlier target fails, while preserving one compiler job, serial Rust
execution and the first failing stage's status.

The later workspace targets expose 13 lowerer fixture failures at the same
explicit class-region projection refusal, plus ten `soac_opt` failures. The
iterator-ownership regression searched for the retired runtime `next` call
instead of the actual structured `IteratorStep`; selecting that real operation
preserves all original source/backedge/ownership assertions and the test passes.
The all-target preflight then completes without a poison cascade: `soac_jit`
has 895 passes and two failures, `soac_lowering` 522 passes and 13 failures,
`soac_opt` 239 passes and nine failures, and `soac_pyo3` seven passes and one
failure. The unselected-module test incorrectly required SOAC ownership even
though the real loader delegates ordinary source. Its original source now
proves that delegation and ordinary behavior; all eight Pyo3 tests pass.

Six field-catalog failures come from the existing consumer not recognizing a
validated local `TakeOperand` for its uniquely stored constant attribute name.
The narrow correction reuses the actual storage layout and duplicate-producer
refusal. A genuine tests-first failure and subsequent **26/26 pipeline tests**
cover the original six positives plus missing-layout, nonoperand, wrong-owner
and duplicate-producer rejection. The three remaining typed cases exposed a
different ownership boundary: a consuming operand is not replayable storage,
and a proposed virtual object is not proof that remaining concrete reads can
lose their allocation. Their original Python sources are preserved. Structured
controls now verify actual allocation, field stores, reads, physical operand
owners and CFG retention, while supported direct-Load scalarization stays
positive. The tests-first gate was **95 passed / three failed**; the narrow
ownership eligibility and independent erasure guard then passed **98/98 typed
tests** and **249/249 optimizer-library tests** on selected optimized `05e18`.
RangeLike's consuming inputs already declined without changes; wrapper and
loop cases exposed unsafe mutation/removal paths. Diagnostic instrumentation
was removed, and exact before/after logs remain under ignored `work/logs/`.

The separate stale-plan audit found that old region labels did not prove
current reachability, and body materialization reused an index after deletion.
Its initial test run had 98 original passes, two genuine refusal failures and
one invalid positive setup: reanalyzing inserted scalar writes correctly lost
the old field-state association. The tests now preserve that refusal and use
the original replayable direct-Load plan for their positive controls; no new
state/replay model was introduced. Direct duplicate-boundary-row coverage was
also added. The corrected tests-first gate has **249 pass, four genuine fail**:
stale CFG, shifted snapshots, duplicate allocations and duplicate boundary rows.
The production guard rechecks actual coordinates and hot reachability, refuses
ambiguous records, and publishes a Rust IR clone only after exact removals and
unchanged unique boundaries. Stale metadata is not repaired. The complete
optimizer-library rerun passes **253/253**, including all 101 typed cases, seven
private virtual-object cases and 26 pipeline cases. Logs are
`work/logs/enforced-final-typed-stale-corrected-before.*` and
`work/logs/enforced-final-typed-stale-materialization-after.*`. Four separate
build-support tests also pass. Scoped formatting checks and the test-target
type-check for all 14 changed workspace crates pass. Pytest collects all
**3,354 tests** without errors; collection is not their execution or the full
gate. This is retained-path correctness, not a new
optimization or performance claim; lifetime/replay extensions remain deferred.

The two JIT fixture corrections preserve original sources and lifetime
observers: native class-comprehension source/capture coverage replaces an
unrepresentable retained-plan witness, and method defaults are prepared during
the class suite before irreversible sealing. The complete class-state family
passes **42/42**. The corresponding ordinary/native/retained behavior gate
passes **34/34**, preserving every original source and lifetime observer.
Consolidation of the 13 source-adapter fixtures is now verified. Their
original source bodies now have three native-data tests covering actual code
trees, source coordinates, raw slots, captures, and region events; the complete
native source-binding family passes **47/47**. This is structured compile-data
evidence, not authority to execute a retained class-region lifetime recipe.
The added six ordinary and six authenticated-native lifecycle cases also pass
**12/12**, preserving the original source bodies and callback/lifetime checks.
Their first run had ten passes and two failures from one incorrect ordinary
observer: the enabled inner comprehension changes a distinct FREE slot to `7`,
while the disabled branch retains the outer marker. Actual ordinary execution,
the pinned upstream 3.15.0a5 emitter rules, and inactive ordinary metadata hooks
establish that expectation without inventing a SOAC lifetime recipe. The two
focused cells and complete family pass after the observer-only correction.
Only then were the unreachable retained-plan tests consolidated into the native
coverage; supported nonregional negatives and native decoder refusals remain.
The complete lowerer suite passes **524/524**, and all **nine** existing native
class-decoder checks pass, including malformed wire, prefix, seed and regional
projection refusals. The complete JIT library now passes **900/900** in a fresh
serial process, without the earlier shared-lock poison cascade. Evidence is retained in
`work/strict-class-lifecycle-native-consolidation-draft/`, the separate provider
and conditional-observer correction packets, and
`work/logs/enforced-final-class-lifecycle-{conditional,full}-after.*`.

The optimized runtime's compiled revision label includes `-dirty` because the
upstream `GITTAG` command selects a Git directory but not the source worktree
when run from the separate build directory. Read-only canonical checks confirm
all 5,565 source/index entries and modes match `05e18`, and the executable,
library, native extension and provenance hashes are unchanged. The diagnostic
does not justify bypassing source checks or restamping the runtime. Its initial
Git describe probe refreshed index bookkeeping; subsequent plumbing verified
unchanged contents, and the final capture left the index bytes unchanged.
The receipt and proposed future explicit-worktree fix are retained under
`work/cpython-build-version-worktree-evidence/` and documented in `README.md`.

The existing 12-case delimiter comprehension/protocol cohort now has a separate
authenticated CPython publication, without changing source bodies or validators.
All 12 native cases, all stock controls and the scheduling tests pass. The first
combined run is **47 pass, 12 fail**: eight retained class-region refusals are
the already documented unrepresented projection, while four failures expose
missing parser-owned source-frame projections for the set/dict-comprehension
targets in two existing retained cases. The native compiler keeps those eager
comprehension locals in the parent frame, while the retained lowering uses a
helper activation and has no parser-owned projection for them. The original
source-frame check correctly refuses that mismatch. The retained cohort now
explicitly checks the four class-region import refusals and two function-entry
frame refusals; the native and stock subjects and validators remain unchanged.
The combined rerun passes **59/59**, including 12 stock and 12 authenticated
native executions, 24 retained outcomes and 11 scheduling checks. Separate
signed publications preserve backend authority. No exception-text xfails,
synthetic unbound slots, relaxed projection checks or silent ordinary fallback
were added. Logs are
`work/logs/enforced-delimiter-native-enrollment-before.*` and
`work/logs/enforced-final-delimiter-native-cohort-after.*`.

The current order is: finish the remaining compatibility and retained-path
regression checks, run `just test-all` against the selected final optimized
interpreter, then finalize evidence and integrate the local-only changes.
Authenticated native loading, function boundaries, pending types and final
decorated-class admission have focused positive and negative evidence above;
that does not replace the full gate. New fixed layouts, direct/virtual dispatch, unchecked
entries, check elimination, closure/suspension ABIs, profile/apply work, and
benchmarks are deferred. The isolated ownership and wire6 drafts below remain
preserved evidence, not prerequisites without a named interpreter-enforcement
requirement and regression. Completing enforcement will not resume optimization.

The final native-coverage audit adds four actual CPython cells to existing
dependency-revalidation, active-call/admission, and hybrid dataclass tests.
The first combined run is **five pass, six fail**. Fresh dependency admission
and active calls retaining their captured pre-admission boundary pass, including
the new native witnesses. Both hybrid dataclasses fail during import in all
three backends, before their unchanged behavior validators execute: the native
slot catalog mistakes a legitimate inherited dictionary field for a new
dictionary/slot conflict. Its inherited-field lookup still requires the old
custom dictionary allocator callback. Ordinary dictionary mode forbids that
callback, while an unchecked inherited dictionary can retain its logical field
catalog with no optional dictionary policy at all. Neither case may lose its
actual dictionary storage or acquire the child's slot-value constraint.
A narrow native correction is committed separately at logical
`b206f0e4fadbe605f075578d43ffbb2acc4f55ea`, with real regenerated-only tip
`023acfa7a20df9d4ac74afbac542587e766339a9`. Its only source delta from `05e18`
is the inherited-field predicate and comment: require a live installed field
catalog and actual dictionary layout independently of the optional allocator.
Two new raw positives genuinely fail on unchanged `05e18`; four existing/new
negative and indexed controls pass. All 32 original slot tests remain unchanged,
and three added methods expand the complete inventory from 695 to 698.
The fresh development build passes **697 cases with one exact debug-only skip**,
all 698 processes exiting zero. Root independently verifies 33 retained
artifacts, three binaries, 1,396 child logs and the exact prior-695-plus-three
discovery set. The same development build also passes the unchanged 14 ordinary
CPython modules: **1,269 pass, eight existing skips and two existing expected
failures**, with all 1,279 discovered identities preserved. Root independently
rehashes 74 artifacts and verifies ordinary no-owner/no-hook observations and
actual binaries. The fresh StackRef-debug gate passes **698/698**, including
the debug-only misuse control and all original shutdown regressions, with no
skips or nonzero child exits. Independent root verification rehashes 34
artifacts, three binaries and all 1,396 logs. After every native consumer
stopped, the locked transaction promoted the exact shared source and verified
the recorded JJ gitlink and all 22 unchanged fixture postimages. The old
runtime and selection were preserved through the fresh actual-root optimized
build and complete native gate described above; normal selection now uses the
verified matching build. Its matching extension also builds successfully.
Original source/artifact/driver identities
are archived under `work/strict-hybrid-dictionary-slot-failure-audit/`; the
genuine failure log is `work/logs/enforced-final-native-compatibility-enrollment.*`.
The unchanged authenticated AFTER run reaches **five pass, six fail** again,
but every hybrid now passes class admission and construction. All six new
failures are the same validator error: `_PyDict_IndexedKeyIndex` rejects an
ordinary dictionary with `TypeError`; its missing-key result is defined only
for an already indexed dictionary. The validators now use the existing
structured `_testinternalcapi.dict_has_indexed_keys` predicate to assert
ordinary storage. The same incorrect witness in the general dataclass validator
is corrected. All class source, policy, field/member/alias checks and the other
test AST entries remain unchanged; no runtime change or exception-based xfail
is involved. The failed run and preimage are retained in
`work/logs/enforced-final-023ac-compatibility.*` and
`work/strict-dataclass-ordinary-storage-witness-correction/`.
The corrected run is **11/11 pass** on the final selected optimized runtime,
including all six hybrid behavior cases and four actual native interpreter
cells. The original field/slot mutation checks run to completion; the native
cells retain actual module/type/function owner and required-boundary witnesses,
with all three lowering/cache/JIT counters zero. Its receipt is
`work/logs/enforced-final-023ac-compatibility-v2.json` (exit zero; pytest
131.60 seconds). All **four general dataclass variants** affected by the same
storage witness also pass (dictionary/slots, retained native/entry interpreter),
including the original ordinary-behavior and generated-owner checks. That
receipt is `work/logs/enforced-final-023ac-dataclass-storage-witness.json`.
Full `just test-all` remains pending.

### Recorded implementation and validation history

CPython's local migration has been promoted: 62 logical commits and a separate
generated top commit `746afdcc078ad997d8295b25f5ad79b5f8824ef2`, tree
`ef904d9094eaa7beb278e22f93d238b074f23c47`, reproduce all 5,559 current source
files' checkout bytes and modes. Interpreter cases were regenerated, not merely
copied from the old patch. The original generation and source inventories are
preserved under `work/cpython-history-migration/`. The complete local clone and
live shared source passed independent raw-byte/mode verification. The fresh
nondebug development build in
`/home/adamh.guest/.local/share/soac/builds/committed-native23-development-01a02587`
passed schema-2 source/build/runtime provenance and import checks, without
changing the saved selection. This is migration validation, not the new native
interpreter execution loop or the final optimized build gate.

Ruff now lives in the shared `vendor/ruff` submodule, with
`https://github.com/adamh-oai/ruff.git` as its origin. Commit
`72cbb3230dce09a0e70ac8dbbb3622dcd8dcb331`, tree
`8320635d91a3fc3dca53805ffc78f5bd57021cc3`, contains 24 logical checker changes,
one explicit archive-portability change, and a separate regenerated lockfile top
commit. All 11,061 files match the earlier validated generation, including the
notebook previously materialized from an upstream symlink. A complete independent
local clone has no promisor/alternate dependency. The rebuilt checker passed
36 exporter, 149 project, and 153 resolver tests. Root and offline Cargo locks
were refreshed without updating compatible external versions. Locked full-graph
verification found 12 runtime and 25 offline Ruff/ty crates, all unique and
resolved from the same verified `vendor/ruff`. An offline metadata probe lacked
a cached crate; its successful retry allowed downloads while retaining `--locked`.

The shared uncached source verifier and build wrappers passed 170 focused
tooling tests. Real JJ fixtures reproduced the parent-index pin mismatch:
verification now reads the recorded JJ `@` tree, fails closed on JJ query
errors, and uses the index only for plain Git checkouts. Both live recorded pins
and complete inventories passed under coordinated native/checker locks before
the locks were released. Evidence is retained under
`work/source-history-migration-integration/`; routine tooling logs are in
`work/logs/`. All 88 maintained patches, the two manifests and the obsolete
native applier were retired after source/build/checker/graph verification;
archived originals remain historical evidence only.
No commits were pushed. Fresh remote checkout/CI availability is deferred until
publication is separately authorized.

The first interpreter core was committed and pinned locally at
`9b99ce3ee139c505560114c9cd50d716d49a8960`, tree
`789b82b205aad3a3af24f21391d3e496023030e2`. It follows the migrated pin with
separate input-ownership, interpreter-hook, stack-correction, fixture-correction
and generated-only
commits. Real case generation initially rejected a dead attribute below a live
function input; publishing the same function token before the callback fixes
that stack shape without adding a reference. Both new structured generator
tests pass. The exact 5,563-file committed source inventory and recorded JJ
gitlink passed the locked promotion checks. The fresh unselected development
build at `interpreter-v1b-development-01a02587` passed schema-2 provenance and
required import checks; optional `_decimal` remains unavailable.

The first isolated native matrix was **22/27 pass**. The five failures exposed
fixture errors: an object-only scanner encounters tagged loop state, an
external unprotected base is supplied to strict construction, two nested
fixture methods capture their own `self` instead of the test object, and a
discarded provisional-class assertion confuses installed pre-seal protection
with final class sealing. GDB evidence and independent process logs are in
`work/logs/interpreter-v1-native-isolated/` and
`work/logs/interpreter-v1-native-attribute-gdb.log`. After the focused fixture
corrections, all **29 focused tests pass**. The complete raw native contract
families run **605 tests: 604 pass and one expected StackRef-debug-only skip**
on the new development build. See `interpreter-v1b-native-focused.log` and
`interpreter-v1b-native-full.log` under `work/logs/`. These are core/legacy
contract results, not pending-type or optimized-build acceptance. Cases cover actual checked binding and
returns, native forwarding/restoration, ownership, failure completion,
generator-throw delivery, and the C++ header.

The actual Rust backend now builds: immutable per-interpreter selection,
separate native module state, source-only native compilation, exact schema-5
source/CALL receipts, and actual compiler-entry counters. Combined test-target
checks pass for all changed crates, including `soac_cpython`. The source adapter
passes all 39 structured tests, including annotation-capture roles and exact
generator-expression argument ranges. All ten retained class-binding decoder
tests pass, as do the nine standalone raw-runtime tests, including refusal to
write inline storage while native policy attachment is PREPARING. That raw
predicate test is not execution of the native attachment transaction. The matching extension's actual loaded path, native
exports and mapped libpython were verified. Two real checker/startup-configured
CPython-backend tests pass, covering public binders, parameter/return checks,
C callers and frame teardown with all three SOAC compilation counters at zero.
The generator case also passes through the real checker and original interpreter
code. Three class-scope nominal tests, the actual factory-method nominal test,
and two common-owner/provider identity and
lifetime tests pass. An unnecessary rejection of original annotation providers
with exact keyword-default dictionaries was reproduced and corrected: both the
empty mapping and hostile unused-key controls now pass without added provider,
code, closure, or globals owners. Native read-only policy preparation does not
call Python or hash/compare those keys; arbitrary unrelated provider metadata
still does not authorize original-source execution.

New validation drivers initially omitted the repository test-helper path; that
fixture problem was corrected. The module witness also incorrectly treated an
absent global as forbidden: the documented append-once policy permits its first
assignment and forbids subsequent replacement/deletion. Its validator now
checks that policy. A separate real lifetime test reproduces the native
module's duplicate globals owner and duplicate GC traversal edge. The focused
ownership handoff passes all five lifecycle tests. It removes the duplicate
module globals owner/traversal edge while preserving escaped functions,
failure/cleanup ordering, and collection. The in-body raw-refcount oracle was
corrected to compare actual ownership edges there, then equal raw counts after
both loaders return: ordinary `exec` temporarily owns three additional argument
references. Twenty module-state, three common GC-state, and fourteen checked-
boundary kernel tests also pass.

Extension staging previously ignored `CARGO_TARGET_DIR`, and isolated
`soac_cpython` tests lacked the selected out-of-tree libpython link path. The
recipes now use Cargo's actual target directory; embedded tests record their
own Cargo profile directory and reject absent matching libraries. Seven Rust
staging/config tests and the absolute/relative target-path recipe test pass,
including stale-library and space-containing-path controls. No stale extension
was accepted as runtime evidence.

The independent pending native candidate is `58dd7d6c8d757b673191d52b85c660abdd0a27e8`,
with a separate logical correction and regenerated-only tip. Its fresh nondebug
development build and 29 fresh-process controls pass: 19 pending/admission,
five linked replacement, four ABI/storage controls and the C++ header. The first
run exposed a replacement preflight still requiring a permanent contract; it
now authenticates the same actual pending construction owner. Fixture corrections
use actual CPython descriptor-exception identity/notes, not an obsolete wrapper-
exception expectation. Evidence and failed runs remain under
`work/cpython-pending-core-commits/`. At that checkpoint, live `vendor/cpython`
and its pin remained `9b99ce3`; the independent candidate was not the selected
runtime.

The subsequent isolated seal candidate is
`d7df14144b36bface3235f0221c7ee36e189d734`. Its actual C release observer first
reproduced the admission gap: allocation was enabled while class and raw-dict
mutation still succeeded. Native now seals the selected namespace before
opening admission or releasing temporary operands. Both new release controls
and the expanded existing native families pass **65/65 in fresh processes**.
The matched build/receipt is recorded in
`work/cpython-pending-core-commits/seal-candidate-ready.json`; this is not a
combined Rust/dataclass acceptance result.

Review of pending dataclass member publication caught a callback-bearing check
at the final resolved-dictionary commit. Its correction separates initial
registered validation from callback-free native revalidation; the previously
frozen implementation must not be used alone. The ordinary dictionary kernel,
member correction, and fourteen-callback CALL bridge are being composed as
logical native commits, followed by regenerated consumers.

The actual native backend also reproduced a nominal timing bug: keyword
equality changed an annotation cell after the old pre-binder snapshot. Moving
the snapshot after ordinary binding passes the new regression and nine existing
native controls. A separate synchronous-release correction preserves temporary
type-edge lifetime on snapshot/commit errors; all ten controls pass again with
that correction and the matching staged extension. This does not claim a
public fixture for every private snapshot/commit failure interval.
The source and before/after receipts are under
`work/strict-interpreter-postbinding-nominal-snapshot-draft/`.

The real early-class admission regression now fails on the live baseline:
an instance is usable during module initialization while its selected method
metadata is not yet finalized. Its exact ordinary-source control passes; the
failure occurs at the actual native diagnostic, not checker rejection. The
pending Rust draft closes admission until method metadata is sealed and keeps
only unresolved module-global nominal leaves in an explicit one-way completion
stage. That draft is not yet validated by a matching combined runtime.

The combined native generation initially rejected the new ordinary-storage
guard before `_RECORD_TOS_TYPE`; the repair preserves the mandatory guard while
keeping the recorder first, without weakening the generator. Structured test
discovery also found seven new native tests under the module's `__main__`
footer rather than its test class. Their unchanged bodies were relocated before
claiming any result. A separate CALL error-path correction fails the same
dataclass invocation before native root release. All these corrections are
separate local logical changes; the combined build and compatibility gate are
still in progress.

The combined build additionally exposed an ordinary-bootstrap crash: reading
`oparg` in generic `CALL_FUNCTION_EX` changed its NOARG classification and
shifted opcode numbers while the bytecode magic stayed unchanged. Existing
source caches then decoded incorrectly. Passing the actual expanded-call
literal zero restores byte-identical opcode IDs; no source cache was deleted
or magic restamped. Selected CALL completion also now publishes the actual
stack before its callback-bearing commit. All sixteen structured generator
checks pass. Candidate `6d87a4718d6e457e78303379b9da0237d6d51c75` has a fresh
verified development build; its exact 397-test inventory, failed rows, and
binary/source evidence are under
`work/cpython-pending-core-commits/ordinary-member-call-v4/`.

That gate found two fixture setup mismatches (generic definition parent and
expanded-call parameter slots), an ordinary/native OOM expectation mismatch,
a real legacy dataclass owner-release regression, and an authoritative-module
dictionary replacement bypass. The adjacent type-namespace C setter is now
covered by separate tests-first probes as well. Proposed native corrections
distinguish selected interpreter completion from legacy C transport and actual
module/type dictionary authority from ordinary borrowed dictionary aliases.
Those corrections pass in the subsequent fresh candidate. Newly reached
fixture failures remain recorded rather than hidden by an xfail or weakened
runtime policy.

The fixture-only successor `1a7d9b1bd61202fee45dedc11201cb20153289d0`
passes **403/403** fresh-process native and structured tests. Its 5,565-file
source digest is
`a5a2dd9c0a0b337bdf5519e31ff736f028307562399c2e4a4ee85779a7bbea91`;
the ABI4/fourteen-callback headers, runtime binaries and all eight root fixture
files remain unchanged across the gate. The final result is
`work/cpython-pending-core-commits/ordinary-member-call-v6/focused-result.json`
(SHA-256 `b523e788521afed3c2cd18e80fb5cad06f741fce233ac96dbdaef5fda497318f`).
The generic decorator observer now explicitly chooses the existing Dynamic
fixture decision after recording the actual operands and incoming edge.
Default-Enforced construction and the unprotected-base refusal still pass;
this does not authorize sealing `typing.Generic`. The OOM test compares the
same actual observer bytecode on both sides, preserving exact equality and
rollback checks instead of treating borrowed-versus-owned stack references as
a leak. The legacy weakref/finalizer regression passes unchanged. Matching
Rust compilation and actual checker/startup-configured admission are separate
pending gates at that native checkpoint.

The locked promotion now pins that exact commit in the recorded SOAC JJ tree
and verifies the live shared source plus all eight fixture postimages, without
changing the saved build selection or pushing. Two borrowed-view coercions and
four stale test enum arguments were corrected before all workspace Rust test
targets passed. The matching extension is
`d9864d2564757b1a015381b99d48bf987bf8dba483a7b40992767fb92b3408a9`;
its actual loaded path, mapped libpython and required exports were verified.
All **five** real checker/interpreter smoke cases pass: dictionary and slots
callback barriers, mandatory metadata sealing before module finality, and
post-binder nominal snapshots including an active call spanning module sealing.
All three SOAC compilation counters remain zero. Evidence is under
`work/logs/pending-v6-native-join-*` and
`work/logs/pending-v6-native-admission-smoke-first.*`.

The next combined compatibility/retained-before run completed **4 pass / 22
fail**, with no skip or timeout, in 288.26 seconds of pytest execution. Ten
new retained-path Pending tests were attempted before their implementation was
published, but later inspection showed that all ten stopped in the helper:
they requested boundary diagnostics that it accepted only for the CPython
backend. They are harness failures, **not behavioral BEFORE evidence**.
Supported native dataclasses unexpectedly remain dynamic; actual
signed artifacts contain Candidate/StdlibDataclass plus `OpenWorld`, while the
new native selector rejects all class uncertainty instead of allowing that
existing deferred uncertainty. Its correction and actual generated-boundary
validation remain pending. Separate fixture issues are the unsupported generic
base expectation, a missing isolated witness import path, and a provider write
that now correctly encounters early method sealing. Their failures are retained
in `work/logs/pending-v6-native-compatibility-and-retained-before.*`; none is
converted into an xfail or a weaker production contract.

The native selector's pure tests-first run reproduces the `OpenWorld` refusal:
**one failure / two passes**. Allowing only that class-level uncertainty then
passes **3/3**, while all other class uncertainties, decorator uncertainties,
unsupported transforms, and disabled policy controls remain rejected. Actual
CALL operands, runtime bases, namespaces, and helper-graph authentication still
run independently. The matching extension rebuild succeeds. All seven saved
dataclass subjects then reach the selected adapter but fail at its compiler
result join; their signed inputs, original drivers and validators remain
unchanged. Logs are
`work/logs/pending-v6-dataclass-open-world-selector-{before,after}.*` and
`work/logs/pending-v6-open-world-runtime-build.*`.
The replay is recorded in
`work/logs/pending-v6-open-world-original-dataclass-replay.*` (39.32 seconds of
pytest execution). The concrete defect is a slot-only catalog reader applied
to the ordinary dictionary-bearing `_FuncBuilder.globals` field, not lost
globals ownership. Its proposed correction consumes the exact native
post-compilation EXEC receipt while retaining actual operand authentication.

The framework witness-path correction changes only the isolated validator's
test-helper import path. All three original native framework controls now pass
unchanged: Pydantic, Django, and SQLAlchemy, in 77.48 seconds of pytest execution.
This is not a fix to framework behavior. The result is
`work/logs/pending-v6-native-framework-harness-after.*`.

The retained Pending implementation and its expectation adapters are now
published and compile. A focused native-commit failure regression reaches the
actual weak registry and fails before cleanup: the terminal entry survives and
would poison the next drain. The correction removes only that entry, preserves
the unrelated live record and failed-type barrier, and the same test passes.
Evidence is `work/logs/pending-v6-retained-terminal-cleanup-{before,after}.*`.
The matching extension is now
`135e6ace8a61524f43b70fc92d146bf0361a55e392530d76ca1d270d5c1d970d`;
its actual loaded path and mapped native library were verified.

The next 32-case retained/native cohort completes **20 pass / 12 fail**, with
no skips or timeouts, in 352.58 seconds of pytest execution. The corrected
nominal/annotation cases, repeated-factory ownership, dynamic framework method
and lexical-function ownership cases, all three framework module-mutation
controls, and the generic-dataclass/descriptor decision pass. Ten failures are
the helper issue described above; the other two are the same ordinary Python
closure-arity mismatch in a dynamic annotation-provider replacement fixture.
Neither is treated as evidence that the corresponding runtime assertion ran.
The log is `work/logs/pending-v6-retained-and-native-compatibility-after-first.*`.

The compiler-result join now consumes the already authenticated native EXEC
receipt instead of using the slot-only catalog reader on `_FuncBuilder.globals`.
Native still checks the actual captured globals immediately after compiler and
cache allocations, before the result callback. Two new ordinary-control and
captured-operand cases fail before the correction and pass afterward. The same
seven retained signed dataclass subjects then complete **six passes / one
failure**: dictionary and slots creation, field/InitVar factories, local nominal
forwarding, selected final `Self`, and original-type lifetime pass. The remaining
post-clear failure reaches the caller's handler and finally block, but a stale
failed-class weak record raises a secondary exception during return-time
completion. The combined after-run is **10 pass / one fail**, including both
dynamic-provider fixture controls. Those controls now use a naturally compiled
replacement with the original closure arity and positional-only shape; the
incompatible replacement remains an explicit negative ordinary-Python control.
The staged extension is
`bd387dc995dccf20bc3435c32952a3b46264b776354f0c33965baee8308dcc9d`.
Evidence is `work/logs/pending-v6-compiled-exec-captured-globals-before.*` and
`work/logs/pending-v6-compiled-exec-original-and-new-after.*`. Original saved
drivers, validators, deployment and artifact bytes were not regenerated.

Expanded field/descriptor enrollment completes **three pass / 15 fail** in
298.61 seconds of pytest execution. Cached-property ordinary compatibility and
dynamic fallback pass on all three execution modes. Nine field cases stop at
obsolete indexed-dictionary diagnostics on deliberately ordinary storage.
Three expose an extra Unicode equality callback compared with the fixture's
contents-only ordinary dictionary. A separate ordinary-only probe on the same
native interpreter reproduces exactly that extra callback on all five setters
when given the original initialization, materialization and deletion history;
copying surviving contents is not an equivalent oracle. The remaining three
fail the original pre-callback descriptor-seal assertion: final-admission
adoption is too late for `__init_subclass__`. Behavior after those failures is
not claimed. Logs are
`work/logs/pending-v6-field-and-descriptor-acceptance-first.*` and
`work/logs/pending-v6-field-unicode-ordinary-history-observation-01.*`.

The independent default-ENFORCED constructor candidate
`ddcae01864f59973f4ff7706f408bf8c8e5e9dc8` closes its early classcell/GC allocation
window using the same native sidecar and active-construction counter, without a
new ABI or state registry. It passes **413/413** fresh-process native/structured
tests on its development build: all original 403 cases, five new constructor
controls, and five existing class-assigned field-test aliases discovered by the
actual unittest loader. All 36 frozen artifact/driver digests were independently
rechecked. Its fresh PGO/LTO optimized build now passes the identical **413/413**
gate, with all 26 frozen artifact/driver digests independently checked. Evidence
is `work/cpython-enforced-barrier-commits/development-v1/ready.json` and
`work/cpython-enforced-barrier-commits/optimized-v1/final/ready.json`. The candidate
remains held, not the live pin or selected build. Inventory comparison with the
earlier 605-case core gate identifies four older families outside this focused
cohort: 18 bridge, 22 descriptor-birth, four dictionary-transition and 238
generator cases. Their additional optimized replay is in progress; the focused
413 are not a substitute for those cases or the full gate.

Tests-first diagnostics prove that the descriptor callback receives the actual
Pending type and authenticated descriptor birth before the original seal
assertion fails, on all three execution modes. The first composed repair passes
all three descriptor cases but regresses eleven dataclass cases: a new second
Input validator ran after the adapter advanced from Prepared to Bound. The
correction removes that duplicate late validation and seals descriptors after
the original actual-input/owner validation, before adapter binding. It changes
no adapter phase permission. The **11 fail / three pass** run is preserved in
`work/logs/pending-v6-cleanup-descriptor-original-and-new-after.*`; its corrected
rebuild succeeds. The corrected gate completes **14/14 pass**, including all
seven unchanged saved dataclass replays, all four new failure cases and all
three descriptor cases. The separate
failed-Apply cleanup removes only the same source/caller/native-owner/graph weak
record; four new ordinary-control tests genuinely reproduce the secondary
return-time error before that change. The two helper-only witness tests also
fail before their helper fix, while all six invalid-expectation controls pass.
After the helper correction, all eight helper controls and all ten retained
Pending cases pass. The latter now reach actual required-boundary C queries on
both retained entries. The registry unit
`native_failed_class_cleanup_keeps_other_graphs_and_scopes` also passes, proving
the scoped removal keeps unrelated graphs, sources and invocations.

Remaining combined gates cover the final native compatibility additions,
optimized and StackRef-debug native validation, promotion of the verified
native candidate, and the complete `just test-all` gate.
No complete interpreter enforcement, optimization, or performance result is
claimed. The old saved build selection remains unchanged and stale against
the newly pinned sources; all current native commands use an explicit matched
build directory.

### Earlier implementation evidence, not a combined interpreter acceptance run

The offline exporter, pre-callback native contract construction, permanent
mutation barriers, checked boundaries, typed layout/dispatch plans, and
dataclass/framework participation paths exist and have focused coverage. They
are **not yet a validated complete combined runtime**. Native21/extension47
identify the historical selected pair, not the newer explicitly matched build
and extension used for the current native-interpreter work.

| Evidence boundary | Latest completed result |
| --- | --- |
| Native23c debug contract suite | 579 passed, no skips |
| Native23c optimized contract suite | 578 passed, one expected debug-only skip |
| Selected CPython regressions on native23c | 52 files / 5,582 cases in each build; 69 debug and 80 optimized skips |
| Rust44l test-target compilation | Eight crates passed; compilation only |
| Rust44g normal-relocation correction | Two actual tests passed, including malformed-receipt refusal |
| Matched native23c/extension44l | Actual mapped cdylib and native-library import proof passed; not selected |
| Independent ownership BEFORE / AFTER | Before: two ordinary passes / nine strict failures; after: ten passes / one frame-OOM exception failure |
| Native scope Rust44l | 23 passed / seven fixture failures; metadata-backed fixture correction is unrun |
| New checker mutable-data cohort | Focused 17/17 passed; full candidate project 165 passed / one class-alias regression; correction unrun |
| Isolated optimizer42c | 269 passed; not the selected full-gate build |
| Latest migrated compatibility cohort | 41 of 52 passed; 11 failures remain |
| Last complete `just test-all` | Failed: Python 603 pass / 89 fail / 104 timeout batches; JIT 824 passed; optimizer 238 passed / 10 failed |
| Historical fixed 97-driver analysis (deferred) | Last result 54 published / 43 rejected; no fresh reanalysis |
| Performance comparison (deferred) | Not measured; not an enforcement acceptance gate |

The chronological evidence below preserves earlier failures and results.
Earlier optimization plans and benchmark requirements are historical; the dated
enforcement-only scope above governs current work. No older checkpoint implies
current completion.

## Hypothesis and evidence

Offline `ty` analysis can describe logical fields, methods, signatures, and
inheritance without per-class SOAC annotations. Feeding those proposals into
actual type construction, and enforcing them for the lifetime of each sealed
runtime object, should allow reusable indexed-field, virtual-method, direct-call,
and dominated-check-elimination optimizations.

The starting compiler has guarded direct calls and indexed dictionaries, but no
authenticated strict artifact, pre-callback construction handle, permanent
mutation barriers, or checked-signature capability. Source write counts, type
annotations, observed dictionary indexes, and registered JIT function IDs are
not substitutes for those contracts. The architectural and acceptance requirements
remain those in `../TYPE_DRIVEN_OPTIMIZATION.md`, `../STRICT_MODULES.md`, and
`../../OPT_GOAL.md`; this record does not narrow them.

## Implementation and compatibility

The earlier full optimization sequence below is historical. The dated
enforcement-only milestone above supersedes its optimization and measurement
steps; it is not the remaining implementation plan.

1. Align the selected language policy and resolve the checker/interpreter source
   boundaries.
2. Export genuine `ty` semantic facts into deterministic, source-bound shards and
   signed, versioned, complete artifact generations. Keep the checker out of the
   runtime dependency graph; share owned contract data and verification code.
3. Authenticate source and policy before execution, preserve contracts through
   lowering, and pass a single-use construction handle to actual CPython type
   allocation before `PyType_Ready` and class callbacks.
4. Enforce module, class, callable, field, and dictionary policies at the shared
   Python/C mutation seams and warmed interpreter paths. Publish no capability
   whose supported mutation paths are uncovered.
5. Check supported synchronous function parameters after normal binding and
   successful return values before publication. Eliminate individual checks only
   with dominating runtime proofs; nominal acceptance is not layout admission.
6. Consume sealed capabilities in explicit typed plans for stable dictionaries,
   genuine native slots, virtual dispatch, and final direct calls. Preserve each
   callee's actual environment and lookup-before-argument evaluation order.
7. Adopt actual standard-dataclass transformations and replacement classes while
   preserving the requested dictionary, slots, defaults, descriptors, and frozen
   behavior. Unknown frameworks remain dynamic before irreversible construction.
8. Complete behavioral/native/structured tests and the full gate, then measure
   authenticated strict execution under the fixed protocol below.

Ordinary Python remains the interoperability boundary. A generic fallback may
withhold an optimization, but cannot unseal a module or undo an installed class
restriction. The supported native boundary excludes non-rejectable mutation of
immutable authoritative dictionaries and malicious native memory writes.

## Benchmark protocol and coverage (deferred)

This protocol is retained for a separately requested optimization phase. None
of its measurements, coverage counts, or thresholds blocks the current
interpreter-enforcement milestone.

- Fixed acceptance selection: the complete pyperformance `all` driver selection
  from the benchmark environment recorded before the first comparison. Preserve
  missing and failed drivers in the coverage denominator; never reduce the set to
  successful intersections for an acceptance claim.
- Fast iteration selection: `chaos`, with three independently started,
  order-alternating stock/strict comparisons; pystone only if `chaos` cannot
  exercise the relevant transformed path.
- Final comparison: `just pyperformance-compare all 3`, extended to identify an
  explicitly strict source overlay and its authenticated contracts. The current
  ordinary-SOAC recipe is not an acceptable substitute.
- Stock source: original workload and algorithm on the same vendored CPython.
- Strict source: the equivalent workload with the strict future feature and a
  recorded project-level policy; exact opted-in modules and source differences
  must accompany results.
- Previous strict SOAC: unavailable at the starting revision. Existing
  ordinary-SOAC measurements are not a previous strict baseline.
- Profiles: generate independently for every candidate and prior strict revision.
- Native profiler: separate explanatory captures only, never attached to headline
  throughput runs.
- Completed/failed benchmarks: not run yet.
- Transformed benchmark, dependency, and standard-library coverage: pending.
- Sealed strict class/function counts, executed hot paths, generic fallbacks, and
  unsupported-framework frequency: instrumentation pending.
- Offline analysis timing, typed-IR growth, native code bytes, and machine-block
  counts: measurement pending.

## Measurements

| Metric | Starting strict baseline | Candidate | Change |
| --- | --- | --- | --- |
| Stock CPython elapsed | Not measured | Pending | n/a |
| Strict SOAC apply elapsed | No strict implementation | Pending | Pending |
| Stock / strict SOAC speedup | Unavailable | Pending | Pending |
| Previous strict / candidate strict | Unavailable | Pending | Pending |
| Optimized typed-IR blocks/instructions | Not collected | Pending | Pending |
| Pre-optimization BlockPy bytes | Not collected | Pending | Pending |
| Apply-mode native code bytes/machine blocks | Not collected | Pending | Pending |
| Offline analysis wall time | No exporter | Pending | Pending |

## Attempt history

### Attempt 1: establish source and enforcement boundaries

- The original six Ruff dependencies pin
  `2d16d8425179c3a235f8c57e72494728ff61a4f7`. Its cached `ty` lacks the requested
  conservative-narrowing options and complete selected Python 3.15 support.
  `ty` diagnostic text and an AST-only scanner were rejected as substitutes for
  genuine structured semantic export.
- The Ubuntu VM root is the shared host checkout, but its `vendor/cpython` is
  overlaid by an ext4 bind mount from
  `/home/adamh.guest/.local/share/soac/cpython`, persisted in `/etc/fstab`.
  Guest CPython is clean at `b607563d68dd972296af89c932af2fb2a0aa6ff2` (indexed
  shared-key dictionaries); host CPython is clean at
  `7ca9e7ad053c24ae40fc68bc931ca1ff8abbc956`. These are not the same physical
  source directory. Guest validation must not be attributed to host CPython
  edits until this boundary is reconciled. The host `Python/` and `python`
  paths resolve to the same directory, demonstrating the case-insensitive
  filesystem constraint behind an in-source Linux build.
- The first focused production-profile regression checks callable mutation
  during argument evaluation, including a global rebound to a decoy. Both code
  and default mutation cases fail in apply mode by calling the rebound global.
  Evidence: `work/logs/type-driven-call-mutation-before.log`, two failed tests
  in `tests/test_regression_function_mutation.py`, 3.59 seconds pytest time.
  The general ordered-child linearizer now captures earlier name reads before
  later lifted work. Four structured linearization tests pass, and the combined
  function-mutation/toolchain Python run passes 17 tests. Logs:
  `work/logs/type-driven-linearization-tests.log` and
  `work/logs/type-driven-focused-python.log`. These establish the ordering fix,
  not a passing strict runtime or a throughput improvement.

### Attempt 2: matched checker and authenticated artifact inputs

- Upgraded all six runtime Ruff dependencies together to
  `d2620d7312875790b114d821721cddf253f66423`. The same pinned source plus tracked
  patches supplies the offline checker. Its downloaded source archive is pinned
  by SHA-256; preparation rejects altered sources and safely materializes one
  bounded internal archive alias that VirtioFS could not extract directly.
- The dependency upgrade initially required an `indexmap` lock update and 89
  lowering API migrations (notably AST suite storage, call ranges, and semantic
  scope APIs). Package-scoped formatting, lowerer test-target checking, and all
  377 lowerer unit tests pass. Newly accepted but not implemented lazy-import and
  unpacking-comprehension forms fail explicitly at source preflight instead of
  silently acquiring eager or non-unpacking behavior. Evidence:
  `work/logs/type-driven-lowering-tests.log`.
- The real patched checker now passes 82 project-library tests and all four
  upstream dataclass regression files. Tracked patches add dialect isolation,
  Python 3.15 selection, semantic export, strict mutation/finality diagnostics,
  suppression accounting, and transitive import dependency collection. The
  observation patch also distinguishes the selected interpreter's actual
  package paths from a misleading installation-prefix layout.
- The shared schema/verifier and filesystem-input tests pass 34 tests, including
  signatures, generation/version/environment mismatches, source identity,
  scoped ignored-diagnostic fallback, conflicting dependencies, and changed
  files, negative import inputs, directories, and symlink targets. Evidence:
  `work/logs/type-driven-contract-review-tests.log`.
- Standalone offline CLI publication has emitted a signed dataclass/function
  module without importing its deliberately raising module body. Five initial
  CLI policy/publication tests pass, covering deterministic shard reuse and
  rejection of tampered, missing, and incomplete generations. Further actual
  checker integration tests and runtime loading remain in progress.
  The first CLI build exposed implicit Cargo workspace membership of prepared
  dependencies; excluding generated `work/` and the standalone tool from the
  runtime workspace fixes that boundary. No native optimization consumes these
  source proposals yet.
- End-to-end analysis found and corrected a `ty` configuration provenance panic:
  ranged options must be parsed with their `ValueSource`, not deserialized as
  detached JSON. It also exposed self-invalidating whole-directory snapshots
  when publication creates its descriptor. Explicit resolver-observation filters
  are being validated against the same cached views the checker actually used;
  silently ignoring changed import inputs is not an acceptable workaround.

### Attempt 3: restore shared CPython source ownership

- The tracked CPython gitlink already selected the guest's `b607563...`; the
  host initialized submodule was stale. Staged the intended clean revision and
  preserved both originals. The initially rejected mount change was performed
  only after the user's explicit approval.
- The first approved promotion hit Git's host/guest ownership check. Its rollback
  restored the exact original mount and `fstab` bytes. Command-local trust for
  the exact shared path (not global/wildcard trust) allowed the retry; helper
  tests cover this path and preservation/rollback behavior.
- Promotion is complete: `vendor/cpython` uses the repository VirtioFS mount,
  a bidirectional host/guest probe verified actual shared writes, and both sides
  report the intended revision. Original host sources are retained under
  `work/cpython-source-migration/host-original`; original guest sources/build
  remain under `/home/adamh.guest/.local/share/soac/cpython`.
- A fresh out-of-tree PGO/LTO build from the shared source completed at
  `/tmp/soac-cpython-build-shared-01a02587`, with source/build provenance recorded.
  `_decimal` is missing in both this build and the preserved original guest
  build; it is not a migration regression. Both use Python's decimal fallback.
  This environment limitation must accompany later benchmark provenance. A
  verified repo-local build selection now lets ordinary `just` commands locate
  the out-of-tree executable. No full gate or benchmark has run against the
  implementation yet.

### Attempt 4: native permanent barriers and callback-time construction

- Source-only CPython patch `0001` installs non-revocable dictionary policies
  outside the public dictionary layout, with GC-visible owners, explicit initial
  validation, staged bulk writes, protected specialized stores, and terminal
  teardown. The isolated pydebug build passes 17 focused policy tests plus five
  CPython regression files (449 tests, two skipped). Logs:
  `work/logs/cpython-dict-policy-tests.log` and
  `work/logs/cpython-dict-regressions.log`. Generated bytecodes are regenerated
  separately rather than being mixed into the source patch.
- The type-development build installs an opaque single-use construction handle
  and GC-owned native class state before `PyType_Ready`, `__set_name__`, and
  `__init_subclass__`. Ten repository-native tests pass for callback observations,
  generic and warmed attribute access, C APIs, class/dictionary/identity writes,
  finality in ordinary/custom-metaclass/native factories, ordinary subclass
  behavior, and immutable GC-visible policy catalogs. These are native primitive
  tests, not yet authenticated production-loader or optimized-runtime evidence.
- New lazy-annotation tests reproduce the initial class-freezing gap. Private,
  provider-scoped cache publication is being tested, including the distinction
  between module append-once semantics and CPython's legitimate recursive class
  cache completion behavior. A public write never receives cache provenance.
- Fixed-prefix storage is a subsequent native patch. The audit found that an
  overflow key can change equality behavior without a dictionary mutation.
  Proposed direct field/default plans must therefore guard alias-sensitive
  dictionaries or use generic checked lookup; sealed layout metadata alone is
  not sufficient evidence. No unguarded indexed-field speedup is claimed.

### Attempt 5: actual construction, ownership, and checked-entry integration

- The genuine offline CLI now passes 19 publication/deployment tests; the
  shared contract crate passes 44 tests, alongside the 84 checker project tests
  and four upstream dataclass files. Reusing a prepared checker reconstructs
  the expected archive/patch tree independently: a forged reuse marker cannot
  authenticate modified source. Runtime verification reads actual loaded
  interpreter/library identities and observed dependency inputs independently
  of the signed deployment's claims.
  Schema 2 now requires semantic explicit/inferred/absent field-annotation
  origin, including bare `Final` handling. The actual schema-2 CLI passes all
  19 deployment tests in `work/logs/ty-field-origin-cli-tests.log`; checker
  callbacks `__init_subclass__`/`__set_name__` are no longer blanket exclusions
  now that pre-callback construction exists. The four custom instance-attribute
  hooks remain excluded.
- Native fixed-prefix dictionaries have one authoritative value array, real
  visible entries, ordinary overflow, and persistent policy after `clear`.
  The finalized storage patch passed 44 focused tests, five CPython files
  (469 cases, two skipped), and 6,000 paired ordinary/indexed mutations. Safe
  successful split-dictionary clear reentry retains its observation/release
  order. Stock-debug reentry cases that corrupt pending clear counts or lose
  finalizers fail explicitly before writing; they are not claimed as matching
  successful stock behavior. Alias-sensitive raw access still needs a separate
  no-lookup-alias proof or a checked generic result.
- The combined native preparation/module/function-owner development build
  passed 50 focused tests and 22 CPython files (1,298 cases, nine skipped)
  before the descriptor-seal extension; the complete `0001` through `0008`
  prefix now passes 57 focused tests and 24 CPython files (1,653 cases, ten
  skipped). Preparation copies keyword
  bindings before `__mro_entries__` callbacks, preserves class-cell/original-base
  order, and binds the Rust owner before type callbacks. Native descriptor
  readers/seals are installed because repeated `property`, `staticmethod`,
  and `classmethod` initialization can otherwise replace supposedly stable
  component functions. Exact descriptor type alone is not immutability.
- Lowering now has an explicit source-bound `ConstructClass` operation,
  preserved by typed conversion, cache ID remapping, and both execution paths.
  Three focused lowering tests pass, including a user-written helper lookalike
  and source-identity archive round trip. The operation is not replayable or
  cross-environment inlineable. Function creation also uses a compiler-recorded
  AST node identity, not helper spelling: both previous name-based recognizers
  were removed. The resulting 384 lowering tests pass, with ordinary-call
  regressions preserving every operand; 216 optimizer tests passed at the
  preceding construction checkpoint. Cache format is version 9 to exclude
  older name-selected creation IR.
- Native owners use an explicit GC-traversed shell. A module wrapper can die
  without terminalizing globals still owned by escaped functions; a sealed
  methodless class can outlive its globals. Detached instance dictionaries
  own only minimal storage policy, not the receiver, type, or module. Pending
  adoption uses weak targets and upgrades one at a time so sealing does not
  retain unrelated closures across callbacks. Actual copied namespaces are
  revalidated without allocating Python lookup keys before callbacks.
- A selected, shared, nondebug development interpreter now runs the exact
  managed native source prefix from the host/guest shared directory. It omits
  PGO/LTO for iteration; benchmark entrypoints require recorded optimized mode.
  Readiness imports critical C test modules before selecting a build. The
  generated cases patch is reproducible with `just regenerate-cpython-cases`.
  Native-linked loader tests pass 10/10 and module tests pass 13/13. The latter
  includes a before/after lifetime regression: dropping PyO3-owned references
  from CPython's `m_clear` without an attachment guard delayed finalizers. The
  native cleanup path now consumes the state slot and releases its two owned
  Python edges synchronously under the caller-owned GIL, without flushing a
  global deferred-reference pool or terminating escaped sealed globals.
- Attribute-write adversarial tests reproduced a string-subclass-name bypass
  through five Python/C entrypoints. Patch `0009` adds attribute provenance to
  the existing one-lookup dictionary transaction, checking both the original
  Unicode payload and canonical stored key after lookup callbacks. Its staged
  build passes 63 strict native cases, 40 dictionary/attribute cases, and 21
  CPython files (1,516 cases, 13 skipped). It is not yet in the selected runtime;
  Rust callback/mandatory-field integration remains pending.
- Public argument/return checking is wired into compiled and interpreted
  entries. Each strict call owns its selected bound defaults and actual cells;
  idle metadata does not preserve obsolete values. Unchecked direct/inline
  admission is disabled for strict targets in primary and profile-only paths
  pending explicit checked-entry proofs. Integrated Rust test targets have
  type-checked; newly added real-checker/startup/runtime tests have not yet run
  against a selected production native build. This is not full runtime proof.
- Automatic dynamic fallback remains for unsupported classes. Explicit
  decorators, dataclass adoption, descriptor construction provenance, checked
  fields (semantic annotation provenance is now exported; transactional runtime
  checks still need integration), virtual
  dispatch, direct calls, and check elimination remain unfinished. Native
  development builds do not replace `just test-all` or performance evidence.

### Attempt 6: exercising the actual runtime boundary

- The first real-checker/startup class test and the first live-default binder
  test both reached module execution but failed before the body: the raw-runtime
  CLIF bundle referenced private `dict_guarded_index` and `indexed_name_matches`
  calls that the codegen backend had outlined despite `inline(always)`. This was
  an actual load failure, not a class-policy or benchmark result. Evidence:
  `work/logs/strict-class-first-runtime.log` and
  `work/logs/strict-keyword-default-before.log`.
- Replaced those two internal checks with the existing self-contained raw
  runtime macro pattern. Five standalone raw-runtime tests pass. A structured
  test now checks callable-symbol closure in the actual emitted CLIF; it passes,
  as do 55 strict Rust tests. Re-executed integration tests get past CLIF loading.
  The standalone crate was not covered by workspace Cargo tests or the package
  formatter recipe. `just test-jit-runtime` and explicit formatter support now
  cover it, and the full gate includes its tests as a serial stage.
- Generated annotation functions now carry their real lexical owner and an
  explicit `AnnotationProvider` role. Function annotation helpers attach by a
  recorded target identity, not a private-looking name prefix. All 385 lowering
  tests pass, including source-helper lookalikes. Class providers are finalized
  by their actual participating class rather than the generic module drain;
  dynamic-framework provider mutation is part of the actual runtime fixture.
- The selected nondebug native generation now includes patches `0009` and
  `0010`. It passes 66 focused native cases; its isolated broader gate passed
  27 CPython files (1,899 cases, 14 skipped). Required checked-function code
  bindings reject even same-object assignment before audit/watchers, separately
  from full sealing. Unannotated/provenance-only functions retain their ordinary
  pre-seal mutation path. Tooling tests pass 46/46.
- The public binder is being corrected against actual CPython controls: only
  missing defaults are looked up, equality exceptions propagate, reentrant
  keyword-default replacement affects later missing parameters, and closure
  cells are captured at successful body entry. These controls pass on the
  selected plain interpreter. Actual suspended-frame and required-code-write
  tests pass in both entry modes (four cases). The keyword-default integration
  case reaches module sealing but exposes rejection of an existing arbitrary-key
  dictionary; preserving and permanently freezing that exact dictionary is
  still being implemented.
  Full runtime acceptance, owned decorator/dataclass adoption, check elimination,
  dispatch, and benchmark evidence remain incomplete.

### Attempt 7: per-execution ownership and native specialized entry

- Actual class execution exposed two integration mistakes: absent class
  keywords were still the lowerer's `None` sentinel, and admission assumed
  native layout descriptors were individually owned by each class. This CPython
  instead caches `__dict__`/`__weakref__` descriptors on the interpreter. The
  runtime now normalizes the omitted keyword operand, and a narrow native
  cached-descriptor identity predicate is staged. Class adoption also now
  consumes the new reference returned by `PyType_GetDict`, rather than leaking
  the class dictionary and its function/global graph.
- A genuine native regression showed unauthenticated strict code could execute
  after a `MAKE_FUNCTION`-created function acquired a valid specialization
  version: `_PUSH_FRAME` bypassed the generic `start_frame` guard. The staged
  fix checks all native frame entries without granting execution merely from
  code flags or source IDs. The combined staged entry/descriptor generation
  passes 73 focused tests and 40 CPython files (2,838 cases, 26 skipped); it is
  not yet the selected runtime for the next Rust/integration gate.
- Strict namespace helpers now carry an explicit third, single-use execution
  argument. Their active environment propagates a Rust-only creation identity
  into new methods and providers; actual type admission compares that identity.
  A same-source method borrowed from an earlier dynamic class cannot qualify by
  source location alone. Four focused structured source/lowering tests pass;
  both-mode genuine factory-transfer regressions await the combined runtime.
- Function annotation providers use weak initial-provider witnesses and are
  adopted through their actual target, not independently by source membership.
  Captured-owner argument/return checks are integrated so an active
  unchecked frame can finish after a permitted code change while later calls
  observe the new implementation. Actual runtime verification remains pending.
- Native patches `0011` through `0013` are now included in the selected v3
  development build from the shared source. Its 77 focused native tests pass;
  the matching isolated pydebug build passes 40 CPython test files (2,843 cases,
  26 skips). The repository venv is verified against this selected interpreter.
  Read-only keyword-default dictionaries retain arbitrary keys and ordinary
  lookup callbacks; they confer neither instance attachment policy nor a
  no-alias proof. These native results do not replace runtime integration.
- Three additional shared-contract tests establish conservative static
  dynamic-class ownership (47 contract tests pass). Known unsupported framework
  methods do not acquire mandatory code protection; nested candidate classes
  and unrelated assigned functions are not demoted by a surrounding dynamic
  class. No already-installed boundary is revoked after a late class decline.
- The first v3 joint gate passed the JIT/PyO3 test-target check but stopped in the
  new copied-layout-descriptor Rust test at a null `PyObject` conversion. The
  other 13 failures were poisoned-mutex fallout; 42 tests passed. Runtime import
  tests had not yet run. The focused failure is being diagnosed before the gate
  resumes (`work/logs/strict-runtime-v3-rust-tests.log`).
  The cause was the test fixture's direct read of the static built-in
  `PyBaseObject_Type.tp_dict`; that namespace is per-interpreter in this CPython.
  Using the owning `PyType_GetDict` accessor fixes the fixture without relaxing
  production checks. The focused test and all 56 strict Rust tests pass after
  the fix. The symbol-closure check also passes and the actual extension is
  rebuilt. Six genuine class cases pass: storage/mutation in both execution
  modes, policy visibility before `__init_subclass__`, detached-dictionary
  lifetimes, automatic unsupported-class fallback, and the shared exception.
  The factory-transfer case stopped at an aliased `locals()` reading the native
  driver's frame. It did not demonstrate acceptance of the foreign method.
- The v3 synchronous-function/field batch completed with 15 passes and 14
  failures (`work/logs/strict-runtime-v3-boundaries-fields.log`). Twelve
  function cases pass, including binding/check order, live defaults, closure
  capture, suspended-frame timing, required code barriers, and arbitrary-key
  keyword-default dictionary preservation. Six failures share an overbroad
  ordinary changed-code fallback rejection; two expose a keyword-subclass
  binder mismatch. Fixes and eight additional adversarial boundary cases are
  written but await the rebuilt extension. Three field cases pass, including
  the profile/apply write path. The remaining field failures identify a
  conservative external-base checker classification, a specialized native
  constructor bypass, and a stale expected value in a multi-mutation fixture.
  Correcting only that fixture's per-iteration expected value passes both
  execution modes against the unchanged v3 runtime and authenticated artifact.
- Calls in semantic class bodies now carry their physical namespace as an
  explicit IR operand, preserved through serialization and typed lowering.
  The native call boundary recognizes actual frame-sensitive builtin identities,
  including aliases and unpacked calls; it never changes a callback's frame.
  Unknown optimized-function locals and dynamic compile/eval/exec remain
  explicitly unsupported rather than receiving the driver's frame. The focused
  source-lowering test passes; broader typed/runtime validation is pending.
- Native patches `0014` through `0016` add explicit call context, an opaque
  mandatory-boundary query, and constructor specialization checks for custom
  `__init__` vectorcall. Three constructor regressions failed before the fix
  and pass afterward. The combined staged gate passes 89 native cases and
  41 CPython files (2,992 cases, 34 skips). The exact v4 source generation is
  now selected in the development build and repository venv; the selected
  interpreter also passes all 89 native cases. It still lacks PGO/LTO and is
  not a performance baseline.
- The v4 runtime checkpoint passes all 45 genuine integration cases (eight
  class, 28 function, nine field) in both native-JIT and forced-entry paths,
  including profile/apply field stores. All 61 strict Rust tests pass, including
  five native-backed sealed-field capability cases. Six class-frame builtin
  integration cases and three focused typed/codegen context tests also pass.
  Logs: `work/logs/strict-runtime-v4-integration-matrix.log`,
  `work/logs/strict-runtime-v4-strict-rust-tests.log`, and
  `work/logs/strict-runtime-v4-class-frame-integration-after-bootstrap.log`.
  The first class-frame integration run exposed an unnecessary dynamic-eval
  t-string probe in runtime bootstrap. Moving that literal to the existing
  untransformed bootstrap preserves native template types without relaxing the
  dynamic-code boundary.
- External-base classification now queries the actual imported strict source
  and its recursively participating MRO. The real checker passes 88 project
  tests and all 20 signed CLI tests. Foreign dataclass-transform bases remain
  conservative until their per-file adapter context is represented.

### Attempt 8: actual-owner field requests and annotation binding identity

- The field consumer selects source-site requests with no offsets or receiver
  type proof. Actual class adoption publishes immutable Rust-only capability
  arrays into each owned function environment; active calls snapshot that array
  before argument callbacks. Raw lookup must pass the actual sealed-owner guard,
  dictionary-alias checks and the fixed-prefix probe; misses retain normal
  attribute lookup. A borrowed hit is retained before receiver/name cleanup.
- Requests are limited to synchronous source functions and known local storage
  candidates. Existing stronger result/access plans are not replaced. Result
  facts are explicitly unknown and synchronized before codegen; the actual
  prepared typed function is revalidated against its authenticated source-site
  catalogue. Structured slot/range/name/source, stale-fact, and genuine
  profile/apply fallback tests now pass at the selected v7 checkpoint below.
- Nominal source identity alone is insufficient: separate executions of one
  class definition can be bound to different annotation aliases. Schema 3 adds
  signed per-annotation lexical binding identities and rejects missing legacy
  fields; 49 shared-contract tests pass. Genuine checker leaf export and
  per-parameter/return actual-target binding are in progress.
- Native annotation replay patch `0017` passes 96 isolated native tests plus
  41 CPython files (2,992 cases, 34 skips). Promotion/build of the corresponding
  selected v5 runtime is in progress. Explicit class-dictionary cells, capture
  projections, and reached conditional-annotation state are still being
  integrated; the isolated native results do not establish runtime compatibility.
- On selected v6, the genuine field-read workload preserves all tested values,
  UNSET/default lookup, alias callbacks, ordinary subclass properties, and
  repeated factory identities, but the optimization assertion fails: no guarded
  field sites are selected. The real checker intentionally records `OpenWorld`
  on classes/sites, and its synthetic `Self` receiver is currently unsupported;
  the planner incorrectly required every uncertainty set to be empty. A guarded
  structural capability must not require a closed world or known field value
  type. The next iteration keeps the actual owner/layout/lookup guards and
  separates optional shape proposals from value proofs. Evidence:
  `work/logs/strict-runtime-v6-field-diagnostic-worker.log` and
  `work/logs/strict-field-sites-pytest773.log`.
- The v7 iteration permits uncertain source-bound shape proposals, including a
  known declaring class for a relational `Self` receiver. It does not create
  value, initialization, subclass-family, or checked-argument facts. Four
  structured field-planning tests pass within the 76-test strict Rust gate.
  The genuine field test now passes profile training, apply behavior and
  emitted-code metadata, then a separate verify replay with both indexed hits
  and original-lookup fallbacks. Two executions of the factory bind independent
  actual-class environments. The field/profile/worker batch passes all three
  cases in 165.24 seconds; this includes setup, not benchmark elapsed-time data.
  Evidence: `work/logs/strict-v7-rust-runtime.log` and
  `work/logs/strict-v7-field-profile-worker.log`.
- Future annotations exposed another real native/lowered mismatch: the pinned
  CPython still gives functions a one-argument string-producing provider, while
  module and class annotations use eager namespace dictionaries. Native `0019`
  obtains canonical annotation strings from the same authenticated AST parse
  and supplies an explicit namespace-only `SetupAnnotations` operation. The
  selected v7 generation passes 107 native cases and the 44-file CPython gate
  (3,111 cases, 41 skips); joint Rust checking, 14 annotation-lowering tests, and
  both real future-annotation execution modes pass. Native strings are not
  authority by themselves: admission consumes the same owned native root.
  Type-alias and type-parameter lazy evaluators remain separate unfinished work.
  Evidence: `work/logs/strict-cpython-selected-v7-native-regressions.log`,
  `work/logs/strict-cpython-future-cpython-regressions.log`,
  `work/logs/strict-v7-lowering-annotations.log`, and
  `work/logs/strict-v7-future-annotations-after.log`.

### Attempt 9: native interpreter identity and class-policy lifetimes

- A genuine two-venv test exposed an omitted runtime prefix check. Both venvs
  use the same executable and library; contracts analyzed against A's package
  were accepted under B and returned B's value `99` instead of A's `41`, with
  A's inputs unchanged. Assigning `sys.prefix` does not change native path
  configuration, while `PyConfig_Get("prefix")` reads that mutable attribute.
  Patch `0018` supplies a per-interpreter native prefix getter, avoiding both
  Python-visible authority and a main-interpreter-only restriction. Its two raw
  prefix tests pass, including independent subinterpreter storage. On selected
  v6, the genuine loader test admits A and rejects B specifically for its native
  prefix, regardless of either process's `sys.prefix` spoof; the final A control
  still returns `41` with its deployment and analyzed inputs unchanged. Evidence:
  `work/logs/strict-prefix-and-type-lifetime-before.log` and
  `work/logs/strict-runtime-v6-prefix-lifetime-classdict.log`.
- The same genuine boundary reproduced a separate lifetime error in both
  execution modes: an escaped `vars()` of a derived, methodless strict class
  retained the class and suppressed its weakref callback, whereas the ordinary
  control collected it and emptied the namespace. Detached instance dictionaries
  already matched the control. Both the native dictionary-policy owner and Rust
  class state held reverse type references; the Rust owner also retained unused
  base references. The isolated native fix replaces the type edge with a
  terminalized comparison address, without moving reference releases. Rust now
  uses per-operation owning type views and no persistent actual-type/base edges.
  The new native-linked owner/dictionary unit passes, and both genuine execution
  modes now match the ordinary lifetime and weakref-callback control on v6.
- Native class-dictionary matching checks the private policy role, owner, and
  live type binding. The initial terminal-dictionary test caught a null-owner
  dereference: terminal cleanup releases the GC owner but retains its immutable
  policy marker. The corrected matcher reports unavailable authority instead of
  dereferencing null or accepting an ordinary fallback. All 100 focused native
  cases and 44 CPython files (3,111 cases, 41 skips) pass in the isolated build.
  The broader gate first found a stale `test_sys` size expectation, reproduced
  on the preceding native generation; syncing that fixture to the actual
  function and heap-type layouts, including portable padding, fixes it without
  changing the native implementation. The source-only patch passes a clean
  5,528-entry replay. Selected v6 passes 76 strict-runtime Rust tests, including
  forged execution-coordinate rejection and terminal class-dictionary access.
  The genuine prefix/lifetime/class-dictionary batch passes all five cases in
  149.09 seconds. Its class-dictionary cases exercise both pre-ready nominal
  binding and post-adoption annotation-cell mutation; neither recorded pointers
  nor source identity alone authorize the lookup. Evidence:
  `work/logs/strict-cpython-identity-focused-native.log`,
  `work/logs/strict-cpython-identity-predicate-gdb.log`,
  `work/logs/strict-cpython-identity-cpython-regressions-final.log`, and
  `work/logs/strict-runtime-v6-prefix-lifetime-classdict.log`.
- This is distinct from the nominal-factory collection failure. That class and
  method survived collection until an unrelated PyO3 entry flushed deferred
  reference releases. Synchronous cleanup of the call's owned nominal snapshots
  fixes both real factory tests on unchanged v5 native code
  (`work/logs/strict-nominal-factory-gc-after.log`). The first prefix test also
  had a fixture-only borrowed-reference mistake: ctypes `py_object` function
  results consume a reference. Using a raw pointer plus an explicit owning
  conversion removes its shutdown crash; production teardown was not bypassed.

### Attempt 10: sealed strict benchmark preparation and evidence

- Ordinary pyperformance drivers execute their measurement loop inside the
  terminal `__main__` block. Merely inserting the strict future would time an
  initializing module; it cannot demonstrate seal-dependent optimizations.
  The fixed `terminal-main-measurement-suffix-v1` preparation keeps definitions
  and setup in initialization, then runs unchanged measurement statements in
  an ordinary copied namespace after the real strict loader returns. Workload
  functions keep their actual strict globals. Syntax reconstruction, global
  rebinding checks, and conservative reflective-access rules reject unsupported
  projections. All 73 currently installed driver source shapes pass this
  preparation preflight; that is not a claim they pass offline analysis,
  execute successfully, or have transformed hot paths.
- Source selection is fixed as `driver-local-static-imports-v1`: the driver and
  transitive static local imports opt in; unimported Python data is byte-for-byte
  unchanged, and dynamic imports, third-party packages, and stdlib remain
  ordinary. The source manifest records original/strict digests, selected
  modules, policy and harness fingerprints. The real CLI analyzes the immutable
  project against the actual prepared benchmark venv before workers start.
  Signing keys, published artifacts, and startup descriptors remain outside the
  analyzed tree. No benchmark worker runs the checker or supplies its own facts.
- The actual upstream runner copies `os.environ`, not the benchmark installer's
  private venv environment. A regression exercises its real `_prep_cmd` boundary
  to verify that the bundle is forwarded and restored. Worker activation
  preserves interpreter options and selects the native startup descriptor;
  missing/stale authority exits before any original user opcode. Raising from
  `sitecustomize` alone was rejected because CPython would ignore it and execute
  an ordinary workload mislabeled as strict.
- A read-only native diagnostic authenticates the actual module owner, globals,
  interpreter, and verified source before reporting seal/source/generation
  evidence. The ordinary worker checks this after import and before measured
  values. It reports native seal snapshots separately from compiled-function
  inventory; neither compilation nor cache files prove execution of meaningful
  hot code. Typed-IR rewrite events now include process IDs to avoid joining
  profile-process sizes to an apply process with equal function IDs.
- Comparisons require original ordinary stock results and explicit strict
  candidate/prior results, matching input fingerprints and strict/harness
  policies per emitted benchmark across rounds. Retired ordinary-SOAC baselines
  are rejected. Seal evidence must match the measured result and exist in every
  round; cache sizes absent from strict lowering are labeled unavailable.
  Replay verifies the same bundle/venv/source/harness and reuses native startup
  and the post-seal entrypoint instead of the original ordinary execution path.
- The source preparation, real upstream environment wiring, fatal startup,
  comparison, replay, and Lima bridge tests pass: 130 tests in
  `work/logs/strict-pyperformance-replay-and-provenance-tests.log`. The genuine
  native diagnostic and ordinary-caller-to-strict-boundary tests pass on v6.
  The first real worker smoke correctly failed because the fixture changed
  `PYTHONPATH` after offline publication. Selecting the worker environment
  before analysis, as the real recipe does, passes actual offline analysis,
  native startup, seal verification, and both pyperf profile/apply processes
  (56.38 seconds for this smoke, not a throughput measurement). Evidence:
  `work/logs/strict-runtime-v6-field-diagnostic-worker.log` and
  `work/logs/strict-runtime-v6-worker-profile-identity.log`. Unit publication
  fixtures are explicitly not runtime authority. No elapsed-time benchmark evidence is
  claimed here: stock, previous-strict, candidate speedups, full-suite coverage,
  optimized size deltas, and the 1.10 geometric-mean target remain unavailable.
- Migrating the existing joined-hot-loop counter regression to a genuinely
  authenticated strict project exposes missing call-target samples. The old
  native function-ID field also authorizes unchecked direct entries and is
  deliberately zero until individual boundary proofs exist. Profiling must
  obtain a separately authenticated observational identity, not re-enable that
  unchecked path. The failing test includes an ordinary callback and an invalid
  strict argument to prevent that conflation. The same worker/profile log
  preserves this negative outcome.
- The separate observational helper now authenticates real source functions
  and bound methods while leaving the unchecked-entry field zero. Its native
  unit rejects a copied function with identical code/globals/defaults/closure.
  The genuine joined-loop regression records strict call targets and an
  ordinary-callback miss, preserves operator/item/field/branch counters, and
  still rejects an invalid checked argument in the trained apply process.
  It and the real sealed pyperf worker smoke pass on v7 in the three-case batch
  above. The preparation/provenance/recipe tests also pass, 131 cases with only
  the separately run real worker excluded, in
  `work/logs/strict-pyperformance-v7-preparation-tests.log`. These results do not
  establish a direct-call optimization or a throughput improvement.

### Attempt 11: actual method families and checked native dispatch

- Added separate immutable runtime method-family identities and receiver rows
  built from the actual participating MRO. Each row resolves the receiver's
  implementation; equal source identities across class-factory executions do
  not equate families. Callable-field shadows and ordinary subclasses miss the
  method row. The metadata has no owning Python edges.
- The typed source proposal and mechanical emitter preserve one lookup before
  argument evaluation and own the captured callee through both continuations.
  A private checked trampoline is selected at lookup, then the public
  vectorcall pointer is compared after arguments. Supported C replacement
  falls back on the same captured callable, without lookup replay. This does
  not publish an unchecked body entry or eliminate boundary checks.
- The first genuine baseline run passes two requested-mode behavioral cases
  but fails the profile/apply case before dispatch evidence: the older exact-int
  region misclassifies captured `offset` as an ordinary local. The fix needs
  an explicit cell-value input, not a different source fixture or a broad
  closure exclusion. Evidence:
  `work/logs/strict-v7-method-dispatch-before.log` (two pass, one fail).
- Two additional real C-setter regressions confirm that the already captured
  function sees vectorcall replacement during argument evaluation exactly
  once. Both then expose retained receivers after an argument exception and
  traceback cleanup in profile mode. The ordinary control releases them; GC
  diagnostics find an otherwise unreferenced bound method holding the strict
  receiver. Replaying both modes with optimization disabled releases receivers
  normally, narrowing the failure to the profiled execution path.
  Evidence: `work/logs/strict-v7-method-vectorcall-retry.log` (two fail),
  `work/logs/strict-v7-method-retention-diagnostic.log`, and
  `work/logs/strict-v7-method-retention-none.log`. These failures remain visible
  and must pass before the consumer is considered implemented.
- The pointer audit also found a validation-path bug: eager function
  preparation called `ensure_clif_vectorcall_compiled` after registration had
  selected the entry interpreter. Both requested modes therefore installed JIT
  trampolines. Earlier paired results remain genuine strict-admission and
  behavior evidence, but do not establish distinct interpreter execution.
  The central preparation guard now preserves the selected entry and the
  authenticated read-only `strict_function_entry_kind` diagnostic is asserted
  by migrated cases. Corrected-path reruns are pending. The same audit narrowed
  the profile lifetime bug to positional argument error continuations that
  omitted the already-owned callable and earlier expression values; these
  now have explicit cleanup before the existing local-frame continuation.
  Native post-call cleanup also follows reverse explicit arguments, receiver,
  and callable order. Paired finalizer and C-setter tests cover those paths.
- The v8 shared-source build adds provenance-only native type-expression
  factories and passes 114 native cases. Its actual shared directory identity,
  complete patched-source fingerprint, selected build, and venv base were
  rechecked. The joint JIT/PyO3 test-target check passes in 6.54 seconds after
  two missing root emitter import qualifications were fixed; 14 annotation
  lowering tests and the separate lazy-alias factory/capture test pass.
  Evidence: `work/logs/strict-v8-joint-check.log`,
  `work/logs/strict-v8-lowering-annotations.log`, and
  `work/logs/strict-v8-lowering-lazy-alias.log`.
- Family kernel and typed decision/ordering regressions are written; their
  joint native/Rust/runtime gate is pending. There are no stock, prior-strict,
  candidate-throughput, or emitted-size measurements for this path yet.

- The first fixed-v8 combined run completed with **21 passed, two failed, four
  deselected** in 248.42 seconds. All twelve nongeneric annotation cases pass.
  Sealed dispatch profile/apply/verify, supported public vectorcall replacement,
  and argument-error capture cleanup pass. Failure evidence remains explicit:
  generic compiled method return/body-error cleanup releases receiver/first/
  second instead of second/first/receiver, and the actual entry interpreter's
  binary operation releases left before right. Evidence:
  `work/logs/strict-v8-dispatch-annotations-cells.log`. This is not a full gate.
- The compiled failure is not a sealed-family guard miss or frame-root leak:
  its reported tracked-root count is zero. Expression linearization hoists
  values into ordinary local owners, borrows them at the consuming operation,
  and retains them until later ascending local cleanup. The correction marks
  expression-temporary lifetimes explicitly, releases direct operands after
  each parent operation and before source assignment/return, and orders pending
  operand cleanup separately from source locals. Root-store, continued-caller,
  exception, and structured acquisition/cleanup tests are being rerun. Cache
  generation 16 records the lifetime category; no name prefix grants it.
- The lifetime checkpoint passes all **221 optimizer units**, all **81 strict
  runtime units**, two compiled/remapped CellValue tests, one pending-temporary
  cleanup test, and one stale prepared-layout rejection test. The first strict
  run was 67 passed/14 failed: its single original failure incorrectly assumed
  a raw escaped type dictionary retains `__module__` after type GC; the other
  thirteen failures were poisoned-lock fallout. A direct selected-CPython
  control confirms that type GC clears that dictionary. The corrected test
  retains all five weak-reference collection checks and now checks the exact
  terminal mutation error. Evidence: `work/logs/strict-v8-opt-unit-gate.log`,
  `work/logs/strict-v8-strict-rust-gate-after.log`, and
  `work/logs/strict-v8-jit-{cells,temporary-cleanup,layout-validation}.log`.
  The updated extension builds in 41.85 seconds. Its combined behavioral rerun
  passes all nine method and twelve nongeneric annotation cases. Both cell
  tests initially stop at real-checker fixture setup: assigning literal None
  made a membership container nullable. A normal argument-taking mutation
  helper preserves the intended dynamic callback without suppressions; both
  cell cases then pass in 57.44 seconds. Evidence:
  `work/logs/strict-v8-lifetimes-behavior-after.log` and
  `work/logs/strict-v8-cell-regions-cleanup-retry.log`. These focused results
  do not stand in for `just test-all` or performance evidence.
- Six isolated generic-alias/function/class cases fail on the selected v8
  runtime after real checker admission and passing ordinary controls. Failures
  distinguish eager alias evaluation, missing real type-parameter captures,
  and unsupported starred TypeVarTuple defaults. This motivates explicit native
  type-parameter scopes, not fabricated callback authority. Four separate
  empty-cell cases also fail exact exception kind/arguments/name comparisons in
  both verified execution paths. Their fix must preserve source-local versus
  free-variable identity through physical cell remapping. Evidence:
  `work/logs/strict-v8-generic-isolated-before.log` (70.16 seconds) and
  `work/logs/strict-v8-cell-errors-before.log` (63.58 seconds).

### Attempt 12: checked native bodies and exact retained-value proofs

- Pacific date: 2026-08-22 PDT. A separate `TypedCheckedCallPlan` selects the
  exact positional body preparation path and nominates original caller
  parameters. Runtime discharge requires a successful existing boundary,
  independent binder ownership of the original value, actual immutable exact
  builtin identity, and identical complete caller/callee predicate. Other
  arguments are checked normally. This is not generalized nominal/annotation
  trust, speculative invalidation, or revocation of any sealed contract.
- Native preparation reuses the actual binder/activation, checks the captured
  public entry after argument effects, preserves the recursion guard, and
  keeps return checking. Unsupported ABI shapes miss before binding; errors
  and body results join the original capture-cleanup path without replay.
  An RAII guard consumes successful preparation on later errors or panics.
  Raw ABI buffers carry explicit native alignment. Argument proofs are retired
  before binder-owned reference cleanup; no new Python lifetime roots are added.
- Added structured source/argument projection and inlining coverage, immutable
  identity/predicate/subclass proof tests, and genuine checker/profile/apply/
  verify behavior tests with per-actual-function counters. Native v9 is selected
  from the shared CPython source, with 122 native cases passing. The joint JIT/
  PyO3 test-target check, 403 lowering tests, 222 optimizer tests, 84 strict JIT
  tests, and 16 cell JIT tests pass. The genuine checked-call tests pass both
  requested entry modes, including apply/verify proof elimination; the combined
  call/dispatch/cell/order cohort is **19 passed in 325.59 seconds**.
  Evidence: `work/logs/strict-v9-{joint-check-4,lowering-unit-gate-3,opt-unit-gate,strict-rust-gate,cell-rust-gate,calls-cells-behavior}.log`.
  Performance and dataclass adoption remain unfinished. No win is inferred from
  the new call shape.
- The next source checkpoint adds source-identity/ABI-selected fixed native
  targets. After binding and checks, code compares the actual activation's
  pinned body with the declared target; equality emits a fixed call using the
  actual environment, while an override takes the same activation's virtual
  body path. No callable/argument work is replayed, no source-equal factory
  environment is substituted, and no closed-world assumption is inferred from
  final annotations. Structured native-call/source tests and real fixed-hit,
  override/final/factory cases are written. The joint Rust test-target build
  and all **86 strict JIT tests pass**. The first actual fixed-target run fails
  during import, before call assertions, because a decorated method's signed
  source starts at the decorator while its native annotation provider starts
  at the definition header. Original parser tokens now supply an explicit
  provider header-line projection, preserved through cache version 19; its
  focused lowering gate is **17 passed**. The next rebuilt runtime must still
  establish fixed-hit behavior. Evidence:
  `work/logs/strict-v9-{header-box-check,header-lowering-tests,header-box-strict-tests,next-runtime}.log`.
- An all-JIT diagnostic reproduced a stack overflow in
  `cross_module_diagonal_set_shell_keeps_resume_targets_plannable`, also failing
  when run alone. The new optional checked-call sidecar had enlarged every
  recursive typed expression even when absent. Boxing that sidecar makes the
  existing deep pipeline regression pass on the normal test-thread stack;
  no stack-size override or semantic exclusion is used. The prior aborted
  all-JIT run has no valid aggregate pass count. Its first independent failure
  was an obsolete ordinary-source counter fixture, not runtime authorization
  that should be restored. Counter execution is moving to the genuine signed
  profile fixture; the Rust admission test instead rejects unauthenticated
  instrumented code. Evidence: `work/logs/strict-v9-{next-all-jit,next-stack-isolated,boxed-deep-test}.log`.
- Fixed-v9 annotations are **20 passed, two failed in 245.97 seconds**. The two
  failures identify a hidden generic scope treating its owned `.type_params`
  cell as an external capture. The narrow producer correction and two focused
  lowerer tests pass. The next runtime reaches the correct ownership but exposes
  a second bootstrap limitation: its synthetic cell helper rejects CPython's
  private `.type_params` name. Indexed valid helper placeholders now preserve
  the original native free-variable tuple; arbitrary invalid names remain
  rejected. Nine bootstrap cases and the **two actual generic replay cases
  pass** with the retained signed generation and exact entry witnesses.
  The dataclass baseline is **six failed in 115.77 seconds**: ordinary behavior,
  requested dictionary/slot replacement, factory/post-init/frozen/order/hash,
  and source-entry controls pass before each missing native-adoption assertion.
  These failures are not waived or converted to expected failures.
  Evidence: `work/logs/strict-v9-{annotation-integration,dataclass-adapters-before,generic-owner-tests-2}.log`.
- The next 19-control-plus-pattern cohort is **36 passed, four failed in
  455.86 seconds**. Pattern captures pass in both actual entry modes. The
  failures are quoted nominal annotations in both modes and two entry-only
  exception-referrer cases. An isolated probe finds exactly one extra tuple
  retaining the exception in the latter. Entry/deopt calls now retain raw
  argument references and use native keyword unpacking without creating that
  tuple; the original two referrer validators pass in both modes (**four
  outcomes**, retained generation). The separately journaled basic controls all
  pass (46 records). The pinned checker now exports quoted nominal bindings
  through its real string-annotation semantic model, with no provider evaluation;
  two actual nominal regressions pass in both modes, including aliases, unions,
  pre-Ready self types and independent factories. Quoted lexical aliases lacking
  a native capture remain explicitly unresolved, not guessed from text.
  Evidence: `work/logs/strict-control19-match-v9-runtime.log` and
  `work/logs/strict-entry-referrers-v9.log`.
- The corrected fixed-v8 compatibility batch is **109 passed, two failed**
  in 576.74 seconds. Module initializers deliberately have an interpreted
  execution policy, regardless of the requested synchronous-function mode.
  Tests now assert that exact policy and separately witness actual function
  entries; suspended cases witness their factory, not resume execution. The
  earlier wrong initializer expectation is preserved, not accepted as either
  mode. The two remaining cases are pattern-guard captures incorrectly absent
  from lexical local bindings, not an intended strict-language restriction.
  Evidence: `work/logs/strict-runtime-v8-entry-policy-cohorts.log`.
- A separate genuine membership probe fails in both actual entry paths:
  container evaluation precedes needle evaluation. The correction retains
  source operand order in IR and swaps only at the `PySequence_Contains` ABI.
  Both failures and the existing passing lifetime cases remain regression
  coverage. Evidence: `work/logs/strict-v8-membership-evaluation-before.log`.
- The constructor-only loader snapshot strategy has its own tracked record
  and three same-runtime/same-deployment pairs. It is not the measurement of
  checked calls or steady-state strict execution; full-suite and prior-strict
  comparison requirements remain unchanged.
- Callback reentrancy exposed additional permanent-contract holes on v9:
  function audit/warning/watcher/finalizer callbacks and dictionary hash/equality/
  watcher/finalizer callbacks could install a seal while an outer write was
  still pending. The isolated prerequisite native fix rechecks the installed
  policy at the actual remaining commit boundary, without replaying lookups,
  rolling back completed pre-seal writes, or revoking a seal. Publication is
  explicitly declined during an already active in-place split-dictionary clear.
  The new baseline contains **18 failing function subcases and 29 failing
  dictionary subcases**; the candidate passes all ten focused methods, all
  **132 native tests**, and **24 CPython files / 2,097 cases / 15 skips**.
  The final header-clean source was frozen, replay-verified against all 5,528
  pinned source entries, and promoted as selected development runtime v10.
  Its actual selected executable again passes all 132 native tests. Original
  v9 binary, library, provenance, and source evidence remain preserved; v10 is
  not PGO/LTO benchmark eligible.
  Evidence: `work/logs/strict-cpython-{prerequisite-focused,dataclass-native-regressions,dataclass-cpython-regressions}.log`.
- A deeper all-JIT diagnostic exposed the remaining typed-node stack cost:
  `InstrTyped` was 1,504 bytes and a debug inline-remapper frame occupied
  225,568 bytes. The two N-queens shell regressions overflowed the normal test
  stack even after boxing the checked-call plan. Sealed field/method requests
  and exact-int branch/return sidecars are now boxed as well, preserving their
  typed semantics. All 645 JIT tests complete without a stack override:
  **641 passed, four failed**. All **404 lowering** and **222 optimizer** tests
  pass. The remaining failures are two scalar plans attached after linearization,
  an obsolete ordinary-closure authorization fixture, and a keyword test naming
  the wrong runtime tuple helper. Corrections are written but not yet rebuilt.
  Evidence: `work/logs/strict-v9-nqueens-stack-{gdb,shapes}.log` and
  `work/logs/strict-v10-{jit,lowering,opt}-unit-gate.log`.
- The pinned 0009 checker and unchanged v9 runtime inputs pass **54 actual
  source-compatibility outcomes** in 278.08 seconds: control flow, declared
  initially absent globals, ordinary interoperability, and checker negatives.
  The retained quoted-nominal pair also passes. Input snapshots before/after the
  batch are byte-identical; these are not results from the subsequently selected
  v10 extension. Evidence: `work/logs/strict-control25-v9-source-provenance-runtime.log`
  and `work/logs/strict-quoted-nominals-v9-after.log`.
- Named keyword calls exposed an extra kwargs dictionary in native/entry
  referrers and reversed entry-only argument cleanup. The baseline is **two
  passed, four failed** in 70.83 seconds. Typed/generic native and entry calls now
  use raw values with only a keyword-name tuple; starred mapping calls retain
  their original protocol. Structured native tests and genuine keyword,
  callable-replacement, exception-identity, and finalizer tests are written.
  Actual execution against the next coherent extension is pending. Evidence:
  `work/logs/strict-named-keyword-v9-before.log`.
- Free functions lacked the optional capabilities already published to
  class-owned methods. The next consumer publishes from actual authenticated
  nominal operands, requires independently sealed native types, and fills only
  absent slots. Foreign callable-member proposals remain guarded and cannot
  turn callable fields into method families. Structured publication/foreign
  request tests and genuine cross-module, override, descriptor, and independent
  factory regressions are written; runtime results are pending.
- The combined v10 compiler checkpoint subsequently passes **648 JIT, 405
  lowering, 222 optimizer, and 51 contract tests**. The corrected stack probe
  measures `InstrTyped` at **672 bytes** (from 1,504), typed extras at 496
  (from 1,136), call access at 56 (from 280), and attribute access at 104
  (from 280). Both deep N-queens tests pass on the normal stack. The earlier
  seven JIT failures were one obsolete ordinary-function authorization fixture
  plus six poisoned-lock followers, not seven independent runtime bugs.
  Evidence: `work/logs/strict-v10-final-soac_jit-unit-gate.log`,
  `strict-v10-decorator-lowering-all-after.log`, and
  `strict-v10-boxed-typed-sizes-after.log`.
- Pinned checker patch 0010 fixes unaliased imported nominal bindings using a
  private import-preserving semantic lookup mode, leaving IDE lookup behavior
  unchanged. **109 project and 24 actual CLI tests pass**; real imported `Box`
  leaves previously omitted by the same-file binding guard are now exported.
  The new actual free-function pair passes on v10, including imported classes,
  later-declared classes, factory-specific nominal targets, indexed field hits,
  direct calls and individual argument-check elimination.
- The first retained fixed/method deployment replay is **two passed, four
  startup rejections** in 83.96 seconds: the old publication used a different
  `LD_LIBRARY_PATH`. These are authentication failures before source execution,
  not behavior failures. Fresh publication under the exact `_pytest-run`
  environment passes all **four fixed-call/temporary-receiver outcomes in
  115.21 seconds**, including actual fixed-body counters and structured emitted
  site/code-size records. No environment validation was bypassed.
  Evidence: `work/logs/strict-v10-{fixed-and-free-runtime,fixed-and-temporary-fresh-runtime}.log`.
- A focused runtime-decline repro exposes a real compatibility defect: supported
  method annotations installed required checks before actual class admission.
  Both dynamic-class outcomes fail, while the pre-callback participating-class
  control passes. The correction keeps class-owned signatures as proposals and
  commits them in native pre-Ready construction, never revoking a boundary.
  Each call captures its active-boundary state before binding callbacks.
  Actual correction validation is pending. The selected descriptor plan also
  still declines explicit wrappers/properties/cached properties; existing native
  component accessors are not a production selection proof.
  Evidence: `work/logs/strict-v10-dynamic-method-boundary-before.log`.

## Dataclass fallback and isolated member-transaction checkpoint

The explicit Prepare/Construct/Apply/Discard fallback now passes both actual
checked-native and entry-interpreter modes (**2 tests, 75.57 seconds**) on the
v10 boundary/collector extension. The genuine signed fixture covers sync and
async success, factory/body/await/application errors, once-only evaluation,
class-argument-before-decorator cleanup, and decorator release despite an
escaped private preparation carrier. This is ordinary decline, not adoption.

The first runtime replay failed in both modes because the new preparation
operation omitted its `eq` keyword label from the module constant pool.
Raw and typed collectors now include named labels as well as expression
operands; the structured regression passes in the **649-test JIT gate**.
The preceding cleanup CFG regression needed completion-discriminator-aware
path traversal, not a producer-order change; the full lowerer gate passes
**405 tests**. The original failures remain in
`work/logs/strict-v10-dataclass-decline-runtime.log` and
`strict-v10-decorator-cfg-diagnostic-2.log`; successful runtime evidence is in
`strict-v10-dataclass-decline-collector-after.log`.

Isolated CPython adapter work separately passes **8 native member transaction
tests**, then **157 native tests and 33 CPython files / 3,243 cases / 24 skips**.
It proves fresh-function-only installation, permanent metadata sealing without
source/JIT authority, exact frozen-hook roles, unchanged final/field/sealed
barriers, replay/copy rejection, post-watcher invocation revalidation, and a
displaced-value finalizer starting a second independently fresh operation.
These kernels are not yet selected production stdlib adoption. Evidence:
`work/logs/strict-cpython-dataclass-adapter-bridges-members-native-regressions.log`
and `strict-cpython-dataclass-adapter-bridges-members-cpython-regressions.log`.
The subsequent direct opcode-member-bridge test also passes: **9 member tests**
within **35 focused tests**, then **167 native tests and the same 33 CPython
files / 3,243 cases / 24 skips**, with unchanged isolated source. This extends
the kernel evidence to the actual context-bearing opcode bridge; it still does
not establish production Rust stdlib admission.

The next source checkpoint moves Prepare to the final raw call boundary,
before factory invocation, rather than wrapping an already returned decorator.
The entry and compiled paths share ordinary operand evaluation and cleanup;
an explicit invocation selector applies only to the enclosing operation, never
its child calls. The structured regression passes in the **650-test JIT gate**,
and the fresh actual checked-native/entry fallback replay passes **both tests**
on staged extension `f21867aa4010d3744259f5651a44219dfc3530ad0f9f6e995d906453ffcbf82b`.
The aggregate runtime log also contains four unrelated lambda-validator
indentation failures after genuine admission, not four runtime successes:
`work/logs/strict-v10-raw-prepare-lambda-nominal-runtime.log`
(6 passes, 4 harness failures, 123.06 seconds). Native-build frozen stdlib recipes are approved as
independent body evidence, with separate actual-environment attestation and no
module execution or persistent Python code roots.

The attestation decision is invocation-scoped semantic verification of the
complete actual stdlib helper graph, not original-helper birth identity.
An equivalent pre-preparation Python function copy can qualify only under
the complete independently verified code/environment/entry check. Fresh
dynamic factories, decorator closures, and generated methods still need exact
native birth records; no shared helper is sealed or made JIT-eligible. Required
field-origin generated constructor checks, default-factory omission provenance,
ordinary/slotted adoption, replacement identities, and the original six
adoption regressions remain pending. No throughput or completion claim follows
from these gates.

The durable checker-0011 dictionary fixture was republished **without running
its test bodies** in 40.67 seconds. Its authenticated shard
`e5adcb75c54aa32fe473adf104dcc8e7cc03ceddd0e14e181f1d0f003d0ef06b`
confirms that `Base.__init__.first` and `Record.__init__`'s `first`, `value`,
and `seed` have explicit supported integer predicates. Synthetic `self` and
the generated `None` return are inferred, while `items: list[int]` is explicitly
unsupported for runtime enforcement. The field's `Factory` fact, not its
signature's ordinary `Value` default, supplies factory provenance. The
generated catalog contains `__init__` and `__replace__`, but not repr/eq;
those missing entries cannot justify invented required return checks.
The generator-field subsequence retains semantic order, including init=False;
appended ClassVars are not an independent constructor-order source.
Evidence: `work/logs/strict-dataclass-artifact-v10-0011-publication.log` and
`work/strict-dataclass-current-signatures.json`.

The registered v11 Rust source checkpoint adds independent recipe snapshots,
weak actual-helper/template witnesses, 22 exact privileged call-site mappings,
separate generated-signature predicates, and deterministic member-role
fragments. All **21 new unit tests pass** within the selected-v11 **674-test
JIT gate** (5.79 seconds), after the test-target check passes in 6.78 seconds.
The fragment test compares all ten generated roles with an
ordinary stdlib transcript. Fresh creation and exact exec-text replay alone
do not authenticate trace-mutated builder inputs, so SOURCE and final member
validation also require the exact selected role fragment. The actual tests
now include trace-body injection, CREATE-watcher code replacement, and
individual annotation-provider/repr-implementation adoption; their runtime
results remain pending. Existing shared helper and user-factory behavior must
stay ordinary. Required generated-entry supplied masks, stock-vectorcall
bypass prevention, whole-conditional value checks, and component adoption need
the next explicit native seam; production dataclass construction still
declines until those requirements can be enforced.
Evidence: `work/logs/strict-v11-descriptor-check-admission.log` and
`strict-v11-descriptor-unit-admission.log`. The first full unit attempt had one
ABI-probe dictionary-key typo and 82 poisoned-lock followers; those were not
83 independent runtime failures. The corrected run is the result above.

Concurrent pytest retention removed the first temporary signed deployment
before replay. The corrected run uses the durable ignored base
`work/pytest/strict-dataclass-decline-v10-next`; retained artifacts still require
the exact authenticated environment, and changed dependencies require genuine
republication rather than weakening the loader.

The next registered ABI2 source checkpoint separates the permanent generated
check delegate from source/JIT ownership, uses preallocated one-use function
birth slots, and binds the single native compiler result through weak code
witnesses. Required checks must be installed before a function weakref is
allocated: GC observers can discover that weakref before CREATE. This review
also found a native unpublished-failure lifetime bug; direct GC deletion was
not safe once a weak or strong function reference escaped. Native cleanup now
tombstones first and releases the fully initialized object normally, preserving
the exception and any escaped terminal object. The compiler's new source
checkpoint is not yet an actual dataclass-adoption result.

A raw selected-v12 compiler probe confirms that repr's decorator expression
contains two ordered CALLs at one span. The structured projection now requires
that exact pair and rejects missing/extra sites; each call still needs its
different callee, operand, and native-birth proof. Evidence is retained in
`work/logs/strict-dataclass-repr-call-sites.log`. Additional unrun-at-this-point
tests cover whole-conditional initializer spans, raw bound-slot predicates,
Field flags without truth callbacks, helper-owner rejection, and a caught
Apply failure followed by another class construction in an already sealed
module. Normal checker 0016 now has actual repr/eq catalog entries with inferred
synthetic annotations and excludes semantic KW_ONLY markers. The older 0011
artifact above remains historical evidence, not the current producer shape.
Fresh-method, selected nominal, and slots/replacement admission still require
the remaining end-to-end protocol and actual six-case adoption gate; no
throughput, full-gate, or completion claim follows from registration.

The subsequent **connected generated-dispatch** checkpoint on selected native
v13 and normal checker 0017 passes the joint Rust check (6.81 seconds), all
683 JIT units (6.08 seconds), and extension build/staging (34.09 seconds).
The staged extension is
`9e46c97e9a7ca12a6989c734396da267b8ff84771b9096c9e80d488cd60e68a5`.
It connects deterministic SOURCE validation, the single generated-code tree,
one-use Created/configured check delegates, repr/annotation component adoption,
and final member validation. The old blanket fresh-method decline is removed;
selected nominal constructor checks and slots remain pre-bind declines.

Its immutable actual non-slots/decline cohort is **20 failed, 2 passed,
4 deselected in 169.78 seconds**. The retained import traces fail before
adapter entry at class-annotation capture correlation: native provider captures
are empty while lowered metadata expects `__classdict__`. Two watcher-test
tails also fail and must be rerun after that producer correction rather than
counted as validated adapter behavior. Input fingerprints before and after
the cohort are byte-identical. Evidence:
`work/logs/strict-v13-generated-dispatch-{check,jit,build,runtime}.log` and
`work/strict-v13-generated-dispatch/inputs-{before,after}.json`.

The next source checkpoint adds actual-C3 inherited field selection and a
minimal declaring-field snapshot interface. Nominal constructor/InitVar checks
must share the original base's snapshot even with `init=False` and disabled
checked fields; neither a changed annotation cell nor mutable `Field.type`
can rebind it. A separate eight-outcome integration fixture passes genuine
normal-0017 checker-only setup in 37.95 seconds; no transformed outcome had run
at that checkpoint.
The SHA-verified schema-4 shard includes six field-owned nominal leaves,
including both InitVars, and the child's inherited fields keep the base's exact
annotation-declaration identities. Its selected-v13 ordinary controls and Ruff
also pass. Evidence: `work/strict-dataclass-nominal-v13-signatures.json` and
`work/logs/strict-dataclass-nominal-{ordinary-controls,v13-checker-before}.log`.
Required nominal-self slots graphs conservatively decline before the original
class binds until an explicit linked-target policy exists: stock cell repair
cannot retarget a previously committed check. This does not waive the remaining
ordinary/external-target slots implementation or any full-suite performance gate.

After the independent provider-capture fix, the next immutable v13 run reaches
the real generated initializer and fails native boundary configuration. Two
minimal cases reproduce independently in both public execution modes:
**4 failed in 58.10 seconds** on unchanged extension `93522fd3…574a`.
One combines a checked integer parameter with an unselected `list[int]`
factory; the other declares a required keyword-only factory before a required
positional factory. The Rust projection incorrectly marked every factory in
the deferred mask while emitting sites only for required predicates, and kept
sites in binder-parameter order rather than actual CALL order. Native
configuration correctly rejected both inconsistent arrays.

The source repair derives the mask from the selected sites and orders each
offset/parameter pair before publishing the native spec; it neither reorders
the generated body nor weakens native validation. Structured ABI-spec tests
cover both defects. The exact before evidence is
`work/logs/strict-dataclass-factory-sites-v13-before.log` and the durable
`work/pytest/strict-dataclass-factory-sites-v13-before` fixture.

The combined repair and nominal-binding checkpoint then passes the Rust
test-target check (8.23 seconds), **689 JIT units** (5.65 seconds), and extension
build/staging (34.61 seconds). Its actual extension is
`7256a81b1b3ea795bcafb6958865ff69505850b830b68ab0f45675a5d3956590`.
On unchanged native v13, normal checker 0017 and Python support, the remaining
non-slots cohort finishes **28 passed, 2 failed, 6 deselected in 219.22 seconds**.
All four factory-mask/site-order regressions and all eight nominal cases pass
in both execution modes. So do omitted-versus-supplied marker checks,
foreign-receiver assignment ordering, actual nonfactory defaults, stock-entry
bypass rejection, whole-conditional tracing, substituted value helpers,
trace-modified source rejection, and caught Apply failure followed by another
construction. The two failures are the ctypes CREATE-watcher test, discussed
below; they are not counted as passes. Before/after input snapshots are
byte-identical. Evidence:
`work/logs/strict-v13-dataclass-remaining-7256-runtime.log` and
`work/strict-v13-dataclass-remaining-7256/inputs-{before,after}.json`.

The separate broad primitive adapter case reaches ordinary behavior and
generated ownership/sealing assertions before failing
`field_index(storage, 'seed') == -1`: an InitVar incorrectly entered the
indexed storage prefix (**1 failed in 49.32 seconds**). The root storage
projection repair and structured regression are source-complete; this is not
yet a successful replay of that broad case. Its exact before log is
`work/logs/strict-v13-nominal-generated-joint-runtime.log`.

Both watcher failures exit with SIGSEGV rather than the intended explicit
adoption error. GDB places the crash in unraisable-error reporting after a
DESTROY watcher entered traced Python with an already-pending exception. The
watcher C API requires saving/clearing/restoring that error before calling
Python; a direct ctypes Python callback cannot do so before its own entry.
A raw v13 control with no strict import or dataclass invocation reproduces the
same SIGSEGV using a temporary lambda destroyed during division by zero. The
untraced ctypes variant loses the original error and exits with SystemError;
the traced no-watcher control preserves ZeroDivisionError and exits normally.
This establishes a fixture C-API violation, not a dataclass cleanup defect.
The replacement regression uses a C-only, error-preserving CREATE capture
helper and mutates the captured function from a later ordinary trace event,
outside the watcher. That corrected regression awaits the selected native
helper; the invalid before outcomes and stacks remain retained in
`work/logs/strict-v13-dataclass-watcher-gdb.{stdout,stderr}.log` and
`work/logs/strict-v13-dataclass-watcher-ordinary-control.json`.

A separate valid, error-preserving C CREATE watcher calls an unowned free
function before runtime attachment. On retained v13, ordinary code runs,
while actual verified strict code raises `StrictRuntimeUnavailableError`
with no body effects despite a null owner and an unset required-boundary bit.
The unconditional strict-code frame guard already closes this hypothesized
publication gap; no native change was needed. The control and exact runtime
fingerprints are retained in
`work/logs/strict-v13-source-function-create-boundary.{log,json}`.

That source-function result does not cover compiler-only bootstrap entries.
A preserving C watcher on selected v14 calls the exact ordinary bootstrap
code before owner/closure installation. The zero-capture placeholder reaches
its raising stub; the closure-shaped placeholder crashes in
`COPY_FREE_VARS` with `closure=NULL`. An ordinary captured-code
`PyFunction_New` control crashes at the same instruction, and the pinned base
has the same NULL-closure-before-CREATE ordering. This is not a v14 native
regression or a successful execution of the compiler helper body. It does
show why the synthetic creation path cannot inherit the strict source-code
guard argument. A private copy bearing only the strict execution-denial flag
rejects both early calls before bytecode, with source identity zero, no owner,
and no required-boundary bit; it grants no source or execution authority.
The existing fully initialized `_PyFunction_FromConstructor` is not exported
by the selected library. The proposed Rust fix therefore guards only private
synthetic code for explicit verified class-helper roles, leaving original
source code and ordinary bootstrap behavior unchanged. These are raw native
controls; actual transformed creation and after-fix tests remain pending.
Evidence: `work/logs/strict-v14-synthetic-helper-create-boundary.json`,
`work/logs/strict-v14-synthetic-helper-create-followup.json`, and
`work/synthetic-create-boundary-probe/synthetic_closure.gdb.stdout.log`.

Slots integration remains unfinished. Its source-only proof now gives the
original and replacement separate construction identities, native owners,
phases, and permanent member witnesses while sharing declaring provenance.
Own source-method parameter, receiver and return checks targeting the class
must join field/InitVar self-target checks in whole-graph pre-bind decline;
inherited base-self targets are not retargeted or blanket-excluded. No slots
adoption, full-gate, or performance claim follows from this checkpoint.

The next connected ABI3 source checkpoint passes the JIT test-target check in
**6.44 seconds** (`work/logs/strict-slots-abi3-source-check.log`). It wires the
actual five-operand slots bridge, independent inherited physical projection,
one-use replacement handle and native association, shared frozen pickle
bindings, cell-repair validation, and paired permanent member publication.
Both weak member owners are ready before native completion; both class edges
are published before active references are released. A replacement-only weak
pending record is inserted before the original without removing the original
on allocation failure. The old single-class completion chain is removed.

Actual combined slots adoption remains pending at this point. New genuine
fixtures cover inherited dictionary/native-slot independence, including an
unchecked base dictionary position shadowed by a checked child slot; frozen
pickle round trips; list-only nested replacements and shared-cell repair;
and an ordinary trace exception after replacement Ready followed by a later
successful construction. The selected-v14 ordinary lifetime control really
executes without strict transformation: shared-cell repair makes the old
class's zero-argument `super()` fail with TypeError, and both old/new classes
collect when their ordinary references disappear. Exact control evidence is
`work/logs/strict-v14-slots-lifecycle-ordinary-control.json`. This is a control
and a source checkpoint, not a successful transformed lifecycle measurement.

The first fixed ABI3 runtime cohort uses extension
`0fbb2262f3ab7548f834b1d1b76707e9e178ee100c901bd898230f0010bcb276`,
selected optimized native v14, and normal checker 0018. It finishes
**6 passed, 4 failed in 116.75 seconds**, with byte-identical input snapshots.
All four broad stdlib dataclass outcomes (dictionary and slots, each in both
execution modes) pass, including generated ownership, component sealing,
inheritance, factories, InitVars and ordinary shared helpers. This is the
actual successful replay of the earlier InitVar-prefix defect, not merely
the source projection repair. Two source-slot outcomes also pass.

The failures identify two independent remaining defects. Both callback
replacement cases reject an already-created, sealed implicit classmethod:
replacement validation incorrectly uses raw class-body Input phase instead
of exact Copied phase. A retained diagnostic confirms the descriptor and
underlying function identities before the rejection. The scoped repair changes
only replacement validation, leaving initial class admission unchanged. Both
unchecked-prefix hybrid cases apply a child's native-slot integer predicate
to its inherited, unchecked hidden dictionary position. The two physical
values must remain independent; the repair must select obligations per
storage location rather than remove the inherited prefix or decline the
supported hybrid graph. These repairs are source-only at this checkpoint.
Evidence: `work/logs/strict-v14-slots-joint-before-runtime.log`,
`work/strict-v14-slots-joint-before/inputs-{before,after}.json`, and
`work/logs/strict-v14-slots-implicit-wrapper-before.json`.

A separate retained nested-replacement lifetime replay is
**1 passed, 1 failed in 11.02 seconds**: entry interpretation collects both
classes, while compiled execution collects the original but retains the
replacement after the last ordinary reference disappears. Four compiled
controls retain it whether neither, either, or both `super()` calls execute,
so the failure does not require those calls or their exception cleanup.
The visible source-method/closure/owner edges form a cycle; that graph alone
does not establish an external owner root. Entering a pure PyO3 diagnostic
and collecting again does not release the class. The remaining native/compiled
edge is under investigation, not excused as an approved lifetime difference.
Evidence: `work/logs/strict-v14-slots-lifecycle-retained-before.log` and
`work/logs/strict-v14-slots-call-lifetime-{owner,pyo3-entry}-neither.json`.

The lifetime investigation then compares actual hardware watchpoints on the
replacement type's refcount. All **65** writes through decorator application
match in the compiled and interpreter runs. Compiled `make_record` next emits
two increments without matching local cleanup; the next **62** native and
module-finalization writes are identical with a constant two-reference
difference. The source and generated annotation cells have equal refcounts
in both modes, and clearing either or both cells does not repair the leak.
This rules out those cell edges and the native replacement transaction as
the source of the retained references. The retained traces are
`work/logs/strict-v14-slots-refcount-gdb-{compiled,entry}.json`.

The actual frozen codegen plan marks both the decorator-result temporary and
the source class local owned on assignment, but classifies them as unbound
after the `try/finally` joins. It consequently omits the class local's return
release and the temporary's delete obligation. A minimal genuine no-method-call
test is **1 passed, 1 failed in 10.85 seconds**, and two generic structured
conditional/finally planning tests both reproduce the incorrect unbound
classification. The scoped fix separates MAY-bound ownership from MUST-bound
load safety, unions exception-prefix ownership, and preserves nullable owned
locals through the existing `Unknown`/`MaybeUnbound` representation. There is
no dataclass or generator exclusion and no native ABI change. Both focused
linked planning regressions pass after the fix in **0.03 seconds**. The joint
linked gate passes **705 JIT tests in 6.27 seconds** and **222 optimizer tests
in 0.19 seconds**. With the rebuilt debug extension on unchanged native v14,
both no-method-call outcomes and both full class-cell/module-drain lifetime
outcomes pass, inside the **10-outcome, 56.90-second** coordinated replay.
Its checker, extension, native and Python-support snapshots are byte-identical
before and after. This closes the actual compiled replacement-reference leak;
the full project gate and performance validation remain pending. Evidence:
`work/logs/strict-v14-slots-no-call-lifetime-before.log`,
`work/logs/strict-v14-maybe-bound-cleanup-plan-before-selected.log`, and
`work/logs/strict-v14-maybe-bound-cleanup-plan-after.log`; the exact frozen
plan is in `work/logs/strict-v14-slots-actual-codegen-plan-pretty.txt`.
After-proof: `work/logs/strict-private-class-capture-{jit,opt}-all.log`,
`work/logs/strict-v14-dataclass-after-fixes-runtime.log`, and
`work/strict-v14-dataclass-after-fixes/inputs-{before,after}.json`.

On the same fixed runtime with normal checker 0019, the complete nominal
dataclass file initially finishes **14 passed, 2 failed in 137.85 seconds**.
All eight existing nominal cases and all six own-source-method self-target
slots declines pass. The named-`self` InitVar cases reach their required checks,
then fail because the test's valid foreign receiver lacks `__post_init__`.
An ordinary v14 control reproduces that AttributeError. Giving only that
receiver fixture its ordinary hook makes both exact retained cases pass in
**10.48 seconds**, with no checker rerun or runtime rebuild. Before, after,
and corrected-replay input snapshots are byte-identical. Evidence:
`work/logs/strict-v14-dataclass-nominal-0019-runtime.log`,
`work/logs/strict-v14-named-self-{ordinary-control.json,retained-fixed.log}`,
and `work/strict-v14-dataclass-nominal-0019/inputs-*.json`.

The next fixed-runtime cohort passes **all 10 outcomes in 115.44 seconds**:
the corrected preserving-C CREATE capture/mutation test in both modes, four
frozen pickle round trips, both checked-prefix/native-member independence
cases, and both failed-slots-Apply/later-construction cases. This is the actual
successful replay replacing the invalid ctypes watcher above, not a claim
that a function was called from inside CREATE. Shared pickle helpers remain
ordinary; after an Apply exception, both escaped type contracts stay installed
and a later construction succeeds. The separate unchecked-prefix routing and
compiled lifetime defects remain red. Inputs are byte-identical in
`work/strict-v14-dataclass-extra-0019/inputs-{before,after}.json`; the exact log
is `work/logs/strict-v14-dataclass-extra-0019-runtime.log`.

The following actual CREATE-invocation family passes **both modes in 51.87
seconds** on the same fixed inputs. A shared test-only C watcher, freshly
compiled against the selected interpreter's own headers/sysconfig, saves the
pending exception before attempting the call. It observes the actual source
MakeFunction with a positive strict source ID but no owner/required bit yet;
early invocation raises `StrictRuntimeUnavailableError` without body effects.
The actual generated initializer already has its permanent creation record
and required check delegate before CREATE; the wrong argument cannot reach
assignment. Both retained exact functions subsequently adopt, enforce their
normal argument checks, accept valid foreign receivers, and resist metadata
mutation. Ordinary invocation and capture-only controls also pass. The helper
does not manufacture either observed function or introduce a production API.
Evidence: `work/logs/strict-v14-dataclass-create-calls-runtime.log`,
`work/strict-v14-dataclass-create-calls/inputs-{before,after}.json`, and the
native fixture build log under its durable pytest base. This closes actual
source/generated entry coverage, not the separate private synthetic-helper
creation or compiled replacement-lifetime work.

The subsequent coherent Rust stage is
`fb968a9f30b66d429f540b09e3a1d197d144ff836541b534d5cc0ddaaef5284f`,
with native v14, checker 0019 and Python support unchanged. The exact retained
publications now pass **all 10 requested outcomes in 56.90 seconds**: copied
implicit-classmethod callback protection in both modes, unchecked inherited
dictionary-prefix independence in both modes, minimal no-method-call and full
shared-class-cell lifetimes in both modes, and both actual source/generated
CREATE-invocation checks. The compiled replacement dies after ordinary caller
references disappear; neither class ownership nor the declared source owner
was retargeted to obtain that result. This closes the two admission/routing
failures and the compiled lifetime failures recorded above. The observation
shim is rebuilt from the selected native headers, and before/after runtime
input snapshots are byte-identical. Evidence:
`work/logs/strict-v14-dataclass-after-fixes-runtime.log` and
`work/strict-v14-dataclass-after-fixes/inputs-{before,after}.json`.

These are bounded correctness results; the full acceptance gate and required
performance comparisons remain outstanding.

### Native-slot owner replay at a reused type address

The native-slot consumer review found a concrete lifetime hole in the Rust
class witness. A caller can retain the exposed native contract owner after
the original class dies, then pass that same owner to the supported native
construction API with no Rust binding callback. A new same-sized class can
reuse the old type address. Its native seal is real, but it is a different
construction; neither that seal nor the reused Rust owner is proof that the
old member offset still belongs to the requested field.

The raw selected-v14 feasibility control reuses the address on its first
allocation, after a weakref proves the original class dead. The native-linked
Rust regression then constructs the original through `FieldCapabilityFixture`,
retains its real owner and sealed-field capability, and creates a replacement
with a differently named member at the old offset. It fails in **0.10 seconds**
with `owner=true, capability=true, wrong_member_read=true`: both Rust admission
and the optional read accept the replacement, and the raw read returns its
unrelated member. Different-address reconstruction is rejected. Evidence:
`work/logs/strict-v14-native-type-owner-address-reuse.json` and
`work/logs/strict-v14-class-owner-aba-before-fixed-fixture.log`. Earlier
`class-owner-aba-before` logs fail only because embedded test Python omits the
repository from `sys.path`; they are not behavioral evidence. The fixture now
temporarily supplies its explicit Cargo-derived repository path and restores
the path afterward.

The corrective design keeps a callback-free weakref to the one actual class
in a pre-reserved GC-visible owner edge. Bound/sealed owner operations must
prove that referent live, and actual-type admission must compare the pinned
referent with the supplied type. The weakref adds no strong class, method, or
globals lifetime edge and cannot be rebound merely by replaying the owner.
No native API or selected runtime change is needed. On pinned GIL CPython,
`PyWeakref_NewRef(actual, NULL)` uses the already-ready metaclass's weakref
offset, the exact builtin weakref allocator, and native list insertion. Its
GC tracking schedules collection at an evaluation boundary rather than
synchronously invoking Python; the existing native slots-replacement bind
already uses this path before the allocation-free Rust binding work.
Post-allocation owner/phase validation and one-way reserved-edge binding are
still required. The native-linked after-fix ABA regression passes in
**0.09 seconds**, rejecting the reconstructed class/old capability on the
same selected v14 native ABI
(`work/logs/strict-v14-class-owner-aba-after-capture-checkpoint.log`). Lifetime
controls and broader actual-consumer checks remain **pending**; this is not
a performance result.

### Reentrant ordinary binding and suspended-frame construction

The actual ordinary-control side of the strict interoperability gate exposes
two separate pinned-CPython lifetime bugs without importing SOAC. A missing
keyword's equality callback can replace `func_kwdefaults` while the binder
still borrows that dictionary. The minimal native regression crashes under
`-X dev` in **1.15 seconds**. A per-lookup owned reference repairs this use-after-free
without snapshotting the entire defaults mapping: later missing parameters
still observe the function's then-current dictionary.

That first candidate passes its focused regression but the broader retained
control still aborts. Keeping the old dictionary alive independently isolates
the second defect. The callback also replaces `__code__`; `RETURN_GENERATOR`
retains the old frame, while `_Py_MakeCoro` rereads the new function code for
both kind and allocation size. Native GDB records **`co_framesize=19` /
`co_stacksize=6`** for the executing code versus **15 / 2** for the allocation input.
Both v14 and the first candidate fail, whereas both no-code-change controls
pass. The three suspended kinds, each tested with a smaller same-kind body
and a synchronous replacement, produce **six failing subcases in 6.642
seconds**. The relevant generator-construction functions are byte-identical
to pinned base `b607563d68dd972296af89c932af2fb2a0aa6ff2`.

Amended patch 0029 passes the active frame's owned code explicitly into the
private constructor, selecting both kind and size from that code. It preserves
generator, coroutine and async-generator behavior, adds no ambient state, and
regenerates the three affected case files. The public SOAC type/callback ABI
and all nine frame-offset probes are unchanged; the private `_Py_MakeCoro`
signature gains the explicit code operand. The old dict-only candidate,
binary files and negative evidence remain preserved.

The isolated debug after-gate passes **both focused tests in 0.183 seconds**,
the full retained ordinary control with default flags and `-X dev`, both
frame-size controls, **262 native tests in 1.206 seconds**, and **37 CPython
files / 3,849 cases in 41.3 seconds** (34 skipped), including `test_call` and
`test_gc`. Two clean patch replays reproduce all **5,541 files** exactly.
Candidate generation is
`49c6194934a56b8aac230fa4ebad751b2724bc6216f104f513f0058990eec3e6`.
The isolated gate leaves the selected v14 inputs unchanged. Subsequent normal
patch promotion preserves the shared source directory's physical identity and
builds optimized v15 out of tree with PGO/LTO and explicit `--no-select`.
After verifying the candidate, selection and venv refresh complete together;
the old v14 binaries and evidence remain preserved. The selected shared-source
gate passes **262 native tests in 0.609 seconds** and **37 CPython files /
3,849 cases in 23.8 seconds** (46 skipped in this optimized build), with
unchanged source/runtime/test fingerprints. Both actual reentrant ordinary
controls pass with default flags and `-X dev`. Startup without
`LD_LIBRARY_PATH` loads the verified v15 library, and the venv's actual native
API/type/frame probes match the candidate. The selected executable SHA-256 is
`8f67b4306c8d8becc525b9137690f7e313b0007914871b1eb36dda63b530d03e`;
libpython is
`c72c607e8b17ca2555db4a2cc6b3a800fa2b19310b8c892d767564b58dd34a54`.
Extension restaging, transformed-runtime afterproofs and the full acceptance
gate remain separate pending work, not implied by this native checkpoint.
Evidence: `work/logs/strict-v14-native-default-lifetime-before.log`,
`work/logs/strict-v14-native-generator-frame-before.log`,
`work/logs/strict-cpython-keyword-defaults-0029-generator-frame-{before.json,gdb.log}`,
`work/cpython-keyword-defaults-amended-candidate/final-gate.json`, and
`work/logs/strict-cpython-v15-final-gate.json`.

## Real framework and captured-cell checkpoint

Selected v10 with pinned checker 0011 and extension
`1a62f777fe79a6af15047a27bee592d9c6a60f76aeec1b8d5ff10b1c36015699`
passes **410 lowering tests**, **650 JIT tests**, and the JIT test-target check.
The fresh signed runtime tests preserve exact before/after interpreter,
libpython, checker, patch, extension, and Python-support hashes in
`work/strict-v10-pydantic-cells/inputs-{before,after}.json`.

- Native lazy `__annotate_func__ = None` on an unannotated class denotes no
  provider. Post-construction validation now accepts that absence marker;
  callable providers still require their actual creation/source owner.
  Both real empty-cache regressions pass, retaining independent method checks.
- Required nominal boundaries now accept the actual type held by the signed
  lexical operand even when it is an ordinary imported or dynamic framework
  type. Four actual regressions pass, including native membership without
  spoofable hooks, distinct factory types, stable adopted targets, and GC
  lifetime. Optional layout/dispatch capabilities still independently require
  a matching sealed class; nominal checks are not eliminated.
- Resolved class-cell references select `ConstructClass`'s cell operand.
  Reserved storage alone was insufficient: the first structured negative test
  exposed that every namespace reserves the slot. Capturing and parameter-
  shadowed lambdas now have distinct decisions. The original class-lambda
  behavior passes both runtime modes with its real native closure metadata.
- Terminal suspended-frame cleanup now releases a preserved cell's storage
  reference, not its contents. A structured generator/coroutine/async-generator
  regression was red before the fix. All four lambda source/default tests
  pass after it, including exhausted generator-expression captures. Two actual
  lifetime tests additionally cover exhaustion, close, throw, finalization,
  and explicit source deletion. The first lifetime validator retained the
  thrown exception's traceback and failed on the ordinary control; releasing
  that test-owned traceback yields **2 passes in 11.16 seconds**, without a
  runtime change or weaker closure assertions. Cache version **21** excludes
  old resolved class-cell and cleanup decisions.
- Genuine Django **5.2.17** and SQLAlchemy **2.0.52** model/database workflows
  pass both modes against ordinary controls. Pydantic **2.13.4** now passes
  class finalization but exposes a separate native policy leak: its dynamic
  model inherits a non-null dictionary factory from a field-less strict
  parent and incorrectly rejects dictionary replacement. That failure remains
  pending, not waived as framework incompatibility.

The two main runtime logs record **6 passes / 2 failures in 173.35 seconds**
(framework/cache) and **6 passes / 2 validator failures in 128.39 seconds**
(closure cells); the corrected lifetime replay passes separately. Evidence:
`work/logs/strict-v10-{framework-cache-after,closure-cells-after,generator-lifetime-replay}.log`.
The prior four dynamic nominal outcomes are in
`strict-v10-raw-prepare-lambda-nominal-runtime.log`.

Host-backed storage filled during this pass. Removing only three reproducible
Cargo incremental-cache directories recovered approximately **50 GiB**; source,
binaries, candidate native trees, test logs, and benchmark evidence were kept.
A preflight free-space check for the build workflow remains a concrete follow-up.
These are compatibility and ownership results, not steady-state performance
or full-gate acceptance evidence.

## Builtin descriptor and real-framework checkpoint

The selected v11 interpreter is an actual **PGO/LTO** build of the shared
vendored directory, generation
`dd2eb8a257f264a8025bfe028bfebe9915ccb58e6ddbc44fb5a972079ddd15b4`.
Its selected native gate passes **191 tests**. The Rust recursion-frame mirror
is compared with six offsets/sizes from that interpreter's C probe rather than
old numeric assumptions. The first JIT run used one wrong probe key and poisoned
the shared native-test lock (**589 passes / 83 reported failures**); fixing that
test-only key and adding an explicit missing-key diagnostic yields **674 JIT
passes in 5.79 seconds**. The offsets themselves matched. Scoped Rust formatting
and `cargo check -p soac_jit --tests` pass. The descriptor source/lowering gate
passes **411 tests**, including structured creation identity, operand order,
class-frame context and archive roundtrip checks.

`ApplyFunctionDescriptor` now represents one signed canonical builtin decorator
applied directly to the original compiler-recorded function creation. The cold
runtime boundary verifies the actual factory, function/code/owner and namespace
execution. Native births have no extra strong function/code/class/global roots.
Input, copied and adopted namespaces are distinct validation phases; explicit
descriptor adoption and required source checks occur after complete copied
namespace validation, before class callbacks. Getter properties retain ordinary
data-descriptor errors and receive neither physical field slots nor protected
instance-method assignment policy. Chained/rebound decorators, reconstructed
wrappers, and same-source wrappers from another execution acquire no authority.
No sealed object is revoked. Cache version **22** includes the new operation.

The actual staged extension
`20189affd081af07ca8e41b7c767eff1a36429933c86f4392dad9625e81d63e2`
passes **six** new signed descriptor outcomes, both execution modes, plus **two**
lifetime outcomes with ordinary controls. They cover callback-time component
seals/checks, evaluation order, explicit ordinary fallback, independent factory
executions, copied wrappers, escaped descriptors and unreachable function cycles.
The first ignored lifetime replay used an invalid fixture-constructor argument
and executed no runtime case; the corrected replay passes. Before/after input
records are byte-identical in `work/strict-v11-descriptors/inputs-{before,after}.json`:
native executable `2df5081444410b5979acd63ecb2f61a2721b927ada209bf5ba808e43ce6bf77d`,
loaded libpython `aa681d159620fae20bc4c0c1101bb92fd7a199d35a46e74284a5d37b9a909e5d`,
and genuine 0012 checker `cabb2f60fa100b44a076f1178b6c7d283345132fc4659c94aae8caa508a85224`.

The combined fresh descriptor/framework run has **10 passes / 2 Pydantic
failures in 195.78 seconds**. The native empty-inherited-dictionary-factory fix
allows model construction; the later assignment failure exposed an independent
SOAC helper bug. `class_lookup_global` scanned all class values for
`__type_params__`, invoking Pydantic's lazy `MockValSer.__getattr__` during an
unrelated annotation lookup. The trace proves recursive model completion and
replacement of the ordinary `Model` validator by a `Prebuilt` validator that
cannot handle assignment. This is not a framework-policy exemption or a native
barrier regression.

A minimal hostile-member annotation fixture fails **both** modes before the
fix. Removing the guessed type-parameter scans restores the namespace/global
lookup operation; lexical generic parameters must use their resolved cells.
The minimal fixture and full genuine Pydantic ordinary/strict comparison then
pass **four outcomes in 24.61 seconds** on the same native/extension and original
signed source fixtures. No Pydantic code was changed. Fresh generic-annotation
checks subsequently pass **24 outcomes in 223.72 seconds**, including both
actual execution modes, private lexical type parameters, a global type name
that must not resolve to a sibling method's type parameter, and hostile
unrelated class members. The fixed-input record is byte-identical before and
after this run in `work/strict-v11-annotation-lookup/inputs-{before,after}.json`.
The run uses the normal 0014 checker executable
`3e6f162c179fdefdda8caf87bcd2ccbcb2737d4a21087a78274ec709ff0e1316`
and Python runtime-support aggregate
`ef15a62b16a087b8101d34be740ed65a6903a7b38493ca9b4064fbdaed914561`;
the native and extension inputs remain those above. The complete gate is
still pending.

Evidence: `work/logs/strict-v11-descriptors-frameworks.log`,
`strict-v11-lifetime-and-pydantic-diagnostic.log`,
`strict-v11-pydantic-rebuild-diagnostic.log`,
`strict-v11-annotation-member-before.log`, and
`strict-v11-annotation-lookup-fixed.log`. These are behavioral/ownership results,
not measured steady-state performance or the final full-gate result.

Native source preparation also needed a durable checkout-byte fix: the first
patch touching a Windows project file must compare against its Git-attribute
CRLF checkout form, not its LF blob. Exact pristine-checkout comparison passes
**31 tooling tests**, rejects noncanonical local edits, and allowed the v11
build without changing the shared mount or overwriting user source changes.

Further C-API replay coverage found a real gap in that descriptor checkpoint:
a class-body callback could obtain a pending descriptor's opaque birth owner
and pass it, the same actual function/owner/code, to `PySoac_NewBuiltinDescriptor`.
The replacement had a new native record, but Rust matched only source and
namespace execution. Both actual modes incorrectly admitted it (**two failures
in 10.58 seconds**, `strict-v11-descriptor-native-reconstruction-before.log`).
The follow-up records a non-reused, per-interpreter native birth ID once in
the Rust producer witness, without adding Python roots. Native v12, its replay
tests and the updated runtime build remain pending here; the earlier eight
positive descriptor outcomes are not proof that this replay boundary is closed.

The subsequent selected **v12** PGO/LTO interpreter closes that replay boundary
in the actual runtime. Its **198 native tests** pass in 0.349 seconds; the
independent candidate also passes 37 CPython files / 3,849 cases (34 skipped).
The full Rust JIT target passes **674 tests in 17.00 seconds**, and lowering
passes **412 tests in 0.95 seconds**, including a structured eager nested
class-cell test that already distinguishes the outer captured cell from the
inner methods' owned cell. No speculative lowerer fix was made.

Fresh genuine offline/runtime descriptor and framework tests pass **18
outcomes in 248.74 seconds**: ten builtin-descriptor outcomes, two ordinary
`cached_property` comparisons, and six Pydantic/Django/SQLAlchemy comparisons.
Both C-API witness-reconstruction cases now decline the replacement class
without revoking the original descriptor's contract. The cached-property cases
preserve the actual stdlib descriptor, miss/hit/assignment/deletion/recomputation,
mutable component behavior, and retained replacement-dictionary identity.
Before/after fixed-input snapshots are byte-identical at
`work/strict-v12-descriptors/inputs-{before,after}.json`:

- Native generation: `93c8ca7655d2afb323512648a56f710c5769d8caf8fbfd2b89f0463738f3d736`.
- Executable: `68dcd29156e66a3803f0fbddfbe092982de850ffca278395e80f674404e85f26`.
- Loaded libpython: `deb74a40e68ae12cb0f22c80191ce6304f0e72dcfaa8a80f9cfc820386766d66`.
- Extension: `9ac52d4cbcc32a4ee30fb82ca9a087ace8b22f641457cc87145bd785f1056a9d`.
- Normal checker and Python support: unchanged from the 0014 annotation checkpoint above.

Evidence: `work/logs/strict-cpython-v12-ready.json`,
`strict-cpython-v12-selected-native.log`, `strict-v12-descriptor-jit.log`,
`strict-v12-lowering-precheckpoint.log`, and
`strict-v12-descriptors-frameworks.log`. Later unbuilt dataclass class-state
plumbing is not part of this staged extension or these runtime results.

The same normal 0014 checker/runtime checkpoint passes **52 actual class and
closure outcomes in 265.54 seconds**, with byte-identical input snapshots in
`work/strict-v11-checker-cells/inputs-{before,after}.json`. This includes the
new implicit nonlocal class-cell case and 25 existing class/closure cases in
both modes, including a previously rejected nested class-cell capture. The
two functionless fixtures establish initializer/admission behavior, not
callable JIT coverage. Evidence: `strict-checker-cells-v11-runtime.log` and
`strict-v11-annotation-lookup-after.log` under `work/logs/`.

### Nominal field-owner checkpoint

The fixed v12 native/extension build and normal 0016 checker reproduce **six
runtime failures in 69.11 seconds** for factory-specific, inherited, and direct
self nominal fields. Offline analysis succeeds. The four factory/inheritance
cases fail because the Holder types have no native contract; the two self cases
fail inside `__init_subclass__` because construction declined before installing
the required owner. This is a field-admission gap, not a checker setup failure.
`work/strict-v12-nominal-field-before/inputs-{before,after}.json` are byte-identical;
the detailed log is `work/logs/strict-v12-nominal-field-before.log`.

The next source checkpoint separates normalized logical layout requirements
from actual GC-owned field policies. Each construction retains its own nominal
targets; ordinary or strict subclasses inherit those actual policies by identity,
without merging equal source ClassReferences from different factory executions.
Direct-self targets reserve GC edges and bind in the native pre-Ready callback.
Other leaves select the authenticated class provider's actual lexical cells, the
actual class namespace, or the source module's globals without evaluating an
annotation. Missing capture/provenance declines before native binding. The
policies retain only required type targets, not the receiver or source module;
a selected self type is necessarily a traversed owning edge.

The selected v13 PGO/LTO interpreter, normal 0017 schema-4 checker and rebuilt
extension now pass **ten actual nominal-field outcomes** in both native-checked
and entry-interpreter modes. These include the original six cases, referent
`__class__` mutation, and detached-dictionary GC lifetime. Field writes do not
publish persistent nominal load proofs: the later return boundary still rejects
a referent whose ordinary type changed. An escaped field dictionary retains its
required nominal targets without retaining the unrelated receiver class, and
the direct-self type/policy cycle is collected after its last external edge is
dropped.

The same fixed-input run deliberately includes three outstanding regressions,
each in both modes: method-only field annotations have no source provider or
closure and need an explicit construction capture; ordinary intermediates can
still gain a strict ancestor through `__bases__`; and an explicit builtin
`object` base still declines because its signed representation does not identify
the builtin. All six fail at their intended behavioral witnesses. The complete
run is **10 passed / 6 failed in 130.40 seconds**; this is not a full green
compatibility gate. The ancestry fix has separately passed 232 native tests and
37 CPython files / 3,849 cases in an unselected candidate. All three `chaos`
classes explicitly inherit `object`, so the builtin-base gap must be closed
before claiming their strict-layout benchmark coverage.

The JIT crate passes **682 tests in 16.59 seconds**, including the field-policy
identity merge test and actual v13 frame-layout ABI assertion. Fixed-input
snapshots are byte-identical at
`work/strict-v13-schema4-fields-baseline/inputs-{before,after}.json`:

- Native generation: `ec20503bfdaea470afd7a51a45add15b85bebd6af93c9805eaa88609fb478c14`.
- Executable: `139ceac964f5607876651b1462b5e498e0fe5f62ec4eaaf103b9a60260b39e92`.
- Loaded libpython: `8fe5a88d9ce755cf48cebccb870bbaa60ee8ff79408084c6b114b0e22e65a1b3`.
- Extension: `47cba134a3819374f2d743054fbc0900b89f53d0c3d1a4f8a536877e09d43471`.
- Checker generation: `9da87a7fe82d8ad233985d9ecaca28c9e2451ca9aaaa0a8a4ba47603d75c2609`.
- Checker executable: `3cbe289b0f9eaf5df53669364c5fb11b64a0779b8a333d813880cbf69ebc041b`.
- Python support: `ef15a62b16a087b8101d34be740ed65a6903a7b38493ca9b4064fbdaed914561`.

Evidence: `work/logs/strict-v13-schema4-fields-baseline.log`,
`strict-v13-schema4-jit.log`, and `strict-v13-schema4-build.log`. The subsequent
connected generated-dataclass dispatcher is not part of this staged extension
or these runtime results.

## v14 private captures and reviewed compatibility

The extension `fb968a9f30b66d429f540b09e3a1d197d144ff836541b534d5cc0ddaaef5284f`
built against unchanged v14 after **705 JIT, 413 lowerer, and 222 optimizer
tests**. Actual method-only field annotations and post-namespace binding reads
pass both modes (four outcomes); six new intervening-function/class-namespace
tests fail at the required native class-owner witness, after their ordinary
closure metadata checks pass. These are missing lexical forwarding, not
successful dynamic fallback. Inputs are byte-identical in
`work/strict-v14-private-capture-stage/inputs-{ready,after-root}.json`.

The actual native-slot profile/apply/verify test passes in **79.37 seconds**:
its structured events identify native object members rather than indexed
dictionary slots, count emitted sites/native bytes, and exercise live slots,
unset/deleted values, and ordinary-subclass lookup fallback. Historical
`indexed_hit` counters are not misreported as dictionary storage.

Reviewed original attribute cases now have **93 passing outcomes in 619.51
seconds**: 23 authenticated strict sources in both modes, six real checker
rejections plus ordinary-code interoperability, and stock controls. The first
async/generator cohort passes 41 outcomes; the next 22 admitted sources pass
42 of 44 runtime outcomes, with TaskGroup exception retention failing both
modes. Its four separately rejected sources pass eight ordinary-interop
outcomes. No validator or original body was edited to obtain these results.
The broad boundary/dispatch gate remains 91 passed / two ordinary-control
GC crashes. Logs and exact inputs are retained under the corresponding
`work/strict-v14-*` and `work/logs/strict-v14-*` cohorts.

Investigation separated two ordinary pinned-CPython defects: reentrant
keyword-default equality could release the mapping during lookup, and changing
`function.__code__` while binding could make `RETURN_GENERATOR` allocate using
the replacement code's smaller frame or wrong generator kind. The second
regression retains the defaults mapping to isolate frame ownership, and fails
all six generator/coroutine/async-generator size/kind subcases before repair.
The amended isolated native candidate passes both focused tests, all **262
native tests**, and **37 CPython files / 3,849 cases / 34 skips**. Two exact
5,541-file patch replays and unchanged public ABI probes pass. Promotion to a
source-authoritative PGO/LTO v15 build is separate; these results do not claim
that selected v14 was repaired.

The next compatibility fixes have explicit negative baselines. Entry
interpretation does not expose the caught exception through `sys.exception()`;
a generator loses its active handler after resumption; and the TaskGroup path
retains a runtime helper frame. That focused gate is **four passed / six failed
in 131.07 seconds**. The new MAY-bound ownership analysis also exposes a
missing materialization for an exception-edge temporary whose producer has
not yet run. The repair must select an explicit proven-unbound operand in the
validated plan, not turn arbitrary missing codegen bindings into null.

A larger reviewed checker cohort exceeded the old helper's 180-second setup
budget. The helper now accepts a per-cohort timeout and retains partial
stdout/stderr on timeout; three focused regressions plus the existing harness
family pass **36 tests**. The retried attribute cohort produced six genuine
checker diagnostics, which were split explicitly instead of being labeled
runtime failures or bypassed.

The handled-state repair now carries nested handler roles through lowering and
uses one original-first region plan for native code and deoptimization. Normal
calls share CPython's actual current exception item; suspended bodies own a
GC-traversed item that is linked only while running. Saved previous values
therefore preserve C-API replacement and the distinction between an empty
current item and its inherited caller. The selected-v15 test-only C probe
confirms `exc_info` offset 136 and the 16-byte, 8-byte-aligned native item (value
offset 0, predecessor offset 8); the native test now compares that actual header
offset with the raw mirror's pinned value. All five handled-state unit tests,
including exact ordered-layout resume identity and reference ownership, pass.

The first actual 22-case selection on staged v15 extension `8f0ba7d5` stopped
after **one native-probe pass and one runtime failure in 79.09 seconds**.
Genuine checker publication succeeded (72.58 seconds), but eager compilation
of a nested coroutine failed Cranelift verification before source behavior
could run. Its preserved-state validation could branch to common cleanup
before the handled-activation SSA value existed. A minimal regression using
the actual lowered coroutine reproduced the same verifier failure; moving
activation initialization ahead of fallible prolog work passes the focused
verifier test and the JIT test-target check. It uses the original pinned resume
argument, not an ambient state or a guessed local. Evidence is preserved in
`work/strict-v15-handled-state-initial-stage/inputs-{before,after}.json` and
`work/logs/strict-v15-handled-prolog-dominance-{before,after}.log`.

A separate retained-publication diagnostic with explicit entry interpretation,
lazy compilation, and background JIT disabled passes the plain handler's actual
exception-identity/restoration checks. Both generator diagnostics still reach
the same failing native resume compilation on their first send, so this is
**one pass / two failures**, not the 22-case afterproof. Runtime inputs and the
signed fixture remain byte-identical under
`work/strict-v15-handled-entry-lazy-recipe-diagnostic/`. An initial replay attempt
omitted the authoritative pytest recipe's signed `LD_LIBRARY_PATH` environment
and was correctly rejected before admission; that failed attempt is retained,
and no environment check was waived. Nested suspension, finalizers, TaskGroup
lifetime, completion context, and actual optimized-to-deopt behavior still
require the repaired extension. No performance result is inferred.

With the prolog repair in extension `023028063f4c6750…`, the fresh genuine
22-case selection reaches **11 passes / one failure in 182.64 seconds** before
`--maxfail=1` stops it. The native layout probe, ordinary/group handlers,
generator/coroutine handlers, and TaskGroup helper lifetime checks pass. The
first nested C-API replacement case passes its exception identity and
restoration assertions but retains its payload at final collection. Replaying
the remaining original tests against the same signed publication gives
**four passes / seven failures in 76.14 seconds**: bare-raise replacement and
finalizer ordering pass; nested payload lifetime, `None` completion context,
and the profiled deoptimization case remain red. These are overlapping
selections, not additive totals. Runtime and fixture/publication snapshots are
byte-identical under `work/strict-v15-handled-prolog-after/` and
`work/strict-v15-handled-remaining-after/`.

The deoptimization failure is an actual abort, not an assertion mismatch. A
fresh profile/apply replay on the same extension after the independently
verified wrapper-owner repair captures `Py_DECREF(0x1)` from
`RuntimeJitDeoptLocal::release_frame_owned_value`; its before/after inputs and
signed publications remain identical under
`work/strict-v15-handled-deopt-backtrace-new-support/`. A structured regression
then reproduces an `I32` Boolean store into the pointer-valued live buffer
without dereferencing it. The narrow repair exhaustively materializes scalar
local representations using the existing Python-value conversion: an
`I32Bool01` selects canonical Boolean objects, while ordinary object and
unbound entries retain their existing ownership. Actual profiled afterproof
still requires the next staged extension; this does not claim the remaining
completion or exception-lifetime failures are repaired.

The subsequent staged extension `c8620129aae0647c…` removes the raw-Boolean
abort but exposes a second real fault: the profiled replay is **one failure in
12.62 seconds**, now a SIGSEGV after its first successful cold continuation.
Native hardware-reference watches show an ordinary `observe` callback being
released by the resumed frame, the exact-positional argument binder, and then
the caller, despite its globals dictionary still retaining that function.
`func_clear` correctly clears the destroyed function's globals; its next call
then crashes in `LOAD_GLOBAL_MODULE`. This is a missing owned reference before
native cleanup, not a CPython function-watcher or exception-state defect.
The fixed-runtime evidence is under
`work/strict-v15-deopt-callback-refcount-gdb/`; a separate codegen DWARF probe
confirms both callback locals are borrowed stack mirrors while the resume
records require owned locals. All runtime/publication snapshots are identical.

The compiler regression must run the actual `Some(SpecializationProfile)` typed
pipeline and materialize its planned constant pool. A manual branch-only
codegen control passed and did not reproduce the defect; the corrected source
handler test returns the right result and observes both callbacks once, but
reduces each callback's reference count from two to one. The cold-capture repair
uses the existing exact-point cleanup-root state to acquire only a missing
owner. Already-owned roots transfer unchanged; a nullable root distinguishes
both sides of a borrowed/owned merge. Its native unit now passes all eight
combinations of untouched, rebound, and conditional arguments with successful
and exceptional cold exits (**one test, 0.14 seconds**). The test restores any
observed deficit before its own cleanup, so its negative assertion does not
itself leave dangling test objects. Logs are
`work/logs/strict-v15-deopt-owner-production-materialized-before.log` and
`work/logs/strict-v15-deopt-owned-capture-after.log`. A new staged profiled replay
is still required; unit success is not that afterproof.

On the same immutable `c8620129…` / support `289cd414…` epoch, the remaining
non-deopt handled selection is **six passes / four failures in 67.66 seconds**.
Nested C-API replacement payload collection, bare-raise replacement, and
finalizer ordering now pass; the four failures concern `None` completion
context. The original cancellation-cause reference-cycle validator still
fails in both entry modes (**two failures in 12.74 seconds**); its ordinary
control has no referrers, while the transformed result retains a `TimeoutError`
referring to the cancellation object. These overlapping diagnostic families
are not additive full-suite totals, and no completion or cancellation repair
is inferred from the callback-ownership fix.

The next source checkpoint replaces synthetic generator/coroutine completion
raises with an explicit `GeneratorReturn` terminator. Lowering pins the return
operand before terminal saved-local cleanup; JIT and entry/deopt execution then
release the remaining activation roots before constructing StopIteration.
None completion uses the pinned CPython `PyErr_SetNone` behavior (including the
caller's current handled exception); non-None completion installs a one-argument
exception directly and preserves tuple/exception return-value identity. Source
raises still follow the existing PEP 479 path. The raw completion helper owns
and consumes exactly one return-value reference and preserves a pending error
during its release. This is coordinated with cache generation 29, not inferred
from a helper name or a rendered block shape. The full lowerer suite passes
**427 tests in 0.27 seconds**, including explicit completion/escaping-error
transport and coroutine exception-region checks; the latter replaces its old
render-string recognizer with structured assertions. On selected v17 the native
completion-helper regression also passes (**one test in 0.01 seconds**), covering
None, scalar, tuple and exception return values, exact caller-context behavior,
owned input release and a pre-existing allocation error. Actual transformed
runtime afterproof still requires the combined extension.

A fixed-runtime GDB capture of the original cancellation validator identifies
four hidden `TimeoutError`-owning exception/abrupt transport locals at the
`gc.get_referrers` call, plus the explicit source cancellation binding. This
is a frame-root lifetime defect, not a failure to restore `sys.exception()`.
The successful diagnostic under `work/strict-v15-cancellation-entry-owner-gdb/`
preserves identical before/after inputs; the original behavioral assertion is
still red. A pure resolved-CFG regression now includes that original source
and follows exact Exception/EnclosingException/AbruptPayload declarations and
edge arguments. It reproduces the missing retirement before the observer;
the companion nested-finally return/raise and source-alias controls pass
(**one failure / one pass in 0.01 seconds**). A source alias deliberately named
`_dp_try_exc_kept` remains distinct from the declared transport set. This is
test-only evidence; cancellation production cleanup is still unchanged.
The corrected gate is
`work/logs/strict-v17-cancellation-transport-corrected-before.log`; initial
observer-location and conservative-path fixture mistakes are retained in the
earlier log and are not counted as a behavioral negative. Separate source
auditing also finds that partial deopt admission
can abandon both an admitted prefix and an unvisited tail of transferred
references, then overwrite a pending scalar-boxing MemoryError. On selected
v17 the constructed native buffer test reproduces reference counts `[2, 1, 2]`
instead of `[1, 1, 1]` and observes a replacement RuntimeError (**one failure in
0.07 seconds**). The repair guards the whole validated slice, transfers owners
atomically into locals, and releases accepted locals on later frame-admission
failure. It rejects a second transfer and preserves the raised error across
decrements. The same test passes both partial and later-admission cases in
**0.02 seconds**; a real-object materialization/one-use/drop control also passes
in **0.02 seconds**, replacing its former fake owned pointer. Logs are
`work/logs/strict-v17-deopt-admission-{before,after}.log` and
`work/logs/strict-v17-deopt-snapshot-owner-after.log`. This repairs deopt
admission, not the separate cancellation transport lifetime defect.
The complementary entry-admission test preserves caller ownership across
three invalid-buffer cases (NULL bound value, duplicate location, and non-NULL
unbound value), then performs the caller's complete cleanup: **one pass in
0.02 seconds**. The combined selected-v17 JIT suite passes **730 tests in
4.43 seconds** (`work/logs/strict-v17-combined-jit-full.log`). The newly added
cancellation retirement regression remains intentionally red at the pure
lowering boundary; this is not a claim that the complete lowerer or full
project gate passes.

The combined v17 extension `f1bb7a7d52d5bc05…` now has actual transformed
completion afterproof. A fresh normal-0020 publication followed by the retained
non-deopt handled selection passes **21 tests in 130.43 seconds**, including
nested C-API replacement, suspended lifetime, finalizers, None/scalar/tuple/
exception completion, close, throw and source PEP 479 behavior. Runtime and
publication snapshots are byte-identical under
`work/strict-v17-handled-remaining-after/`. Earlier attempts to replay old-v15
publications were correctly rejected because their offline authority included
the old `.venv/bin/python`; those preflight failures are not runtime negatives.
The unchanged sources were genuinely republished for v17, not patched in old
manifests or admitted by waiving the executable check.

The same extension's profiled callback-ownership replay now passes all behavior
and exception-identity assertions but fails its final handoff-counter assertion:
the existing instrumentation configuration creates no deopt-entry counters.
An independent exact-function GDB replay proves **two actual admissions** to
`dp_jit_deopt_resume`, each at record 10 with seven live values and the original
depth-one ValueError handler, followed by normal inferior exit. That diagnostic
passes in **7.90 seconds**, with identical inputs and publications under
`work/strict-v17-deopt-native-handoff-after/`. This is the native callback repair
afterproof; it does not turn the still-red permanent counter regression into a
pass. Permanent cold-handoff observability must use the final resume plan and
an explicitly owned counter sidecar, not resize already-published scalar arrays.

Cancellation remains a real two-entry failure after fresh v17 publication
(**two failures in 81.12 seconds**, including analysis). New selected-native
controls show why clearing all stale transports only after handler exit is
insufficient: an inner exception's finalizer sees the restored outer handler,
and replacing the current exception through the C API releases the unaliased
original before the next source statement. A maintained three-case/two-entry
family reproduces all six mismatches on the unchanged v17 artifact (**six
failures in 73.62 seconds**): inner destruction instead sees the caller,
fallthrough destruction waits until function return, and C-API replacement
retains its original until after the observer. The native control log is
`work/logs/strict-v17-native-exception-transport-finalizers.log`; the fixed
actual negative is `work/strict-v17-handler-finalizers-before/`. The next
source repair separates stable handler identity from caught-value transport,
retires only exact declared transport values after semantic use, and replaces
the interpreter's frame-long raised-error cache with an edge-owned snapshot.
That new source work is not yet compiled or staged at this checkpoint.

### v15 compiler and fixture checkpoint

The shared-source PGO/LTO v15 interpreter is now selected; its native gate and
exact build identities are recorded above. The explicit unbound-entry plan
passes `cargo check -p soac_jit --tests` and all three focused MAY-bound tests,
including the original yield/delegate source after the production late
expression-linearization pass. This is a structured compiler afterproof, not
yet a transformed-runtime replay of that source.

Policy selection now shares the enabled write/generated-parameter decisions
between contracts, lowering, and runtime binding. It retains eligible original
dataclass declarations even for `init=False`, because a later signed subclass
can generate parameters from those inherited fields. Disabled ordinary
method-only annotations remain edge-free. The shared contract family passes
**58 tests**; the refreshed normal 0019/schema-5 binary is
`70ef0306dfe9de005e2056b4a8faccacb4565108f9a9cafd7f8002edcd7fa1a7`.
That epoch's 31 actual CLI tests pass. A subsequent exact-provenance correction
requires a matching own class member and annotation definition before treating
an instance annotation as an inheritable dataclass generator field; unrelated
method-only annotations must not create hidden lifetime edges when field checks
are disabled. The 58-test family covers absent/mismatched declarations and
`init=False` inheritance. Its refreshed normal executable is
`3cfabf191d23b7550c290afb0be812843bed90d973c1d335b27898e8bf9edaf1`;
the corresponding full 31-test CLI replay passes. The subsequent 0020
source-literal-safe checker is recorded below.

Namespace-handle forwarding/teardown and nested exception-context work remain
in source integration. New tests require exact native ownership and enforce
the real field predicate before checking escaped-handle cleanup. No runtime
afterproof is claimed yet: the v15 venv refresh removed the staged extension,
and an inadvertently broad helper-test selection stopped its sole actual
benchmark smoke case at `ModuleNotFoundError`. The explicitly isolated helper
and benchmark-source unit selection passes **68 tests in 0.69 seconds**.

The legacy source opt-in now reuses the benchmark's AST-equivalent future
insertion, retaining module docstrings instead of placing an import before
them. Its focused regression passes. Three old validators now separately
check real stock behavior for `dir`, implicit `exec`, and closure `eval` rather
than expecting SOAC's explicit unsupported-frame error on stock CPython.
The generic-typing validator no longer skips because of an obsolete CPython
checkout path; its original annotation/base assertions pass on selected v15.
These fixture corrections are disclosed, not counted as unchanged-validator
runtime evidence.

## Unsupported source-literal escape boundary

The pinned Ruff parser decodes surrogate escapes as U+FFFD before either
lowering or type inference sees their values. Native `surrogatepass`
materialization cannot recover the original string after that substitution.
The normal 0019 checker (`3cfabf191d23b7550c290afb0be812843bed90d973c1d335b27898e8bf9edaf1`)
actually signed both direct `Literal["\ud800"]` and an ordinary imported
`Alias = Literal["\ud800"]` as exact U+FFFD parameter and return contracts,
with no uncertainty. Independent manifest/signature/shard verification
confirmed the two wrong facts. Stock v15 returned U+D800, distinct from the
genuine U+FFFD and six-character raw-backslash controls. The preserved invariant
test has **two failing subcases**; a separate tracked admission replay has
**six genuine failures and three stock passes in 39.24 seconds**, with matching
before/after native, checker, and support fingerprints. Those six failures are
successful publication of unsupported source, not setup or runtime errors.

The bounded repair is explicit rejection, not lossless surrogate support.
New `soac_source::{validate_source_literals, UnsupportedSurrogateEscape}` uses
actual Ruff tokens and prefix flags, inspects exact escape units, and retains
the original byte range. Selected strict source and main lowering reject
active surrogate escapes before consuming decoded literals. Actual second
annotation parses validate their own tokens. In SOAC analysis, ordinary
dependencies with unsupported source literals withhold exact string-literal
facts as uncertainty; f/t-string interpolation operands are still analyzed.
Ordinary Python execution and genuine U+FFFD/raw-backslash values are unchanged.
BlockPy cache 27 prevents reuse of prior lossy lowering results. The three
legacy validators no longer approve U+FFFD substitution as SOAC behavior.

The isolated shared-token matrix passes **five tests**, the isolated real
checker project passes the **four focused export/dependency tests**, and the
actual `ruff_db` second-annotation parser passes its focused test. The normal
0020 checker now passes **34 actual CLI tests**, including selected-source
rejection and imported-alias suppression of the false replacement-character
contract. On the recovered persistent v15 interpreter, the same tracked
admission/stock replay now passes **all nine cases in 7.50 seconds**: six
unsupported strict sources reject before publication, while the three stock
Unicode controls retain their exact original values. Complete before/after
checker, native and support snapshots are byte-identical. The two genuine
transformed positive controls now also pass **in 16.97 seconds** on the
`8f0ba7d5` extension, with actual checked-native/interpreted entries and identical
before/after native, checker, extension, support and test-source snapshots.
They preserve surrogate argument identity, distinguish U+FFFD from raw
backslashes, exercise f/t-string controls, and return ordinary-module surrogate
values without transformation. Evidence is preserved under
`work/strict-surrogate-before-0019/`,
`work/logs/strict-source-literal-signed-invariant-before.log`,
`work/logs/strict-source-literal-tracked-before.log`, and
`work/logs/strict-ty-source-literal-*.log`; the normal-checker receipt is
`work/strict-ty-0020-v15-ready.json`, and the tracked afterproof is
`work/logs/strict-source-literal-tracked-after-persistent.log` with snapshots
under `work/strict-source-literal-tracked-after-persistent/`.

The first runtime replay had **two test-expectation failures**, not a missing
mandatory predicate: it assumed that `Literal[...]` belongs to the shared
`supported_annotations` boundary subset. Independent signature verification
confirmed the exact U+FFFD/raw facts, while the shared policy explicitly leaves
literals dynamic. The corrected assertion requires identity-preserving
surrogate round-trips rather than adding a new language restriction. The
existing optional-literal native predicate test separately passes with exact
U+FFFD/raw positives and U+D800 negatives. Both the mistaken assertion and its
afterproof are retained in
`work/logs/strict-source-literal-controls-after-persistent.log`,
`work/logs/strict-source-literal-controls-policy-after-persistent.log`, and
`work/logs/strict-v15-unicode-optional-literal-guard.log`.

Transferable lesson: validate lossy front-end representations before signing
their semantic predictions. A runtime decoder or a guard on only the selected
module cannot repair information already lost in an imported alias or a second
annotation parse.

## User-written annotation callback replay

The environment-matched v15 legacy review reached an actual replay defect in
`annotationlib_fakeglobals` in both execution modes. Its user-written
`annotate` callback is an authenticated `SourceFunction`, not a compiler
`AnnotationProvider`; the replay resolver wrongly required the latter role.
Stock `annotationlib.call_annotate_function(callback, Format.STRING)` returns
`{'x': 'int'}`, while both strict entries rejected it with "capture schema
requires an authenticated annotation provider". The earlier legacy run's
`LD_LIBRARY_PATH` observation mismatch was a correct preflight rejection, not
this runtime failure. Fresh publication under the actual runtime environment
preserves that distinction; no signed authority was rewritten.

The bounded Rust change leaves the native replay API and compiler-provider
capture rules intact. A user callback must retain its exact live owner, native
code, and closure layout. Every code node in its immutable native tree must
correspond by pointer to one admitted source callback or annotation provider;
source bodies with selected required checks and unrepresented/class/generic
construction nodes fail explicitly. The root's native required-boundary flag
is checked before and after code allocation, and its closure tuple is pinned
across callbacks to prevent address-reuse substitution. Only fresh ordinary
replay code is returned: no source ID, strict flag, JIT metadata, or source
mutation authority is copied.

The existing annotation replay integration family now includes user callbacks
with defaults, real lexical cells and nested lambdas, exact ordinary controls,
ordinary replay-code metadata, and checked/nested-checked/class/copied-function
negatives. Its genuine staged `8f0ba7d5` baseline is **4 failures / 2 passes in
35.82 seconds**, with native, checker, extension, support and test snapshots
unchanged. Evidence is retained in
`work/logs/strict-v15-user-annotation-replay-before.log` and
`work/strict-v15-user-annotation-replay-before/`.

The combined post-change source passed **420 lowerer and 716 JIT unit tests**,
then built and actually imported extension `023028063f4c6750…`. Its recorded
native, checker and Python support inputs are byte-identical to the prior
`8f0ba7d5` epoch; only the extension changed. The new six-case replay family
passes **6/6 in 38.27 seconds**, and the original signed legacy fake-globals
publication passes both entries in **5.01 and 5.10 seconds** without
reanalysis or changed source/authority. The existing compiler-provider
regressions also pass **8/8 in 53.93 seconds**, covering module/function/class
capture layouts, nested forward references and original-code copy rejection.
Every runtime gate verified its own before/after input equality. Logs are
`strict-v15-user-annotation-replay-after-02302806.log`,
`strict-v15-retained-fakeglobals-02302806.log`, and
`strict-v15-user-annotation-replay-providers-02302806.log` under `work/logs/`;
the actual build/import receipt is
`work/strict-v15-replay-extension-ready.json`. The unrelated annotation-only
native-position mismatch was fixed and replayed separately, not waived by
this callback policy.

A checked custom annotator that requires synthetic globals remains an explicit
limitation: the current `CodeType` result cannot install a checked boundary on
annotationlib's temporary function. Supporting it needs an explicitly owned
ordinary checked replay entry. This change does not silently drop required
checks, broaden CodeOnly admission, or optimize synthetic-globals execution.

## Completion evidence still required

### Persistent native recovery and source-safe checker

Moving inactive caches from the shared checkout to guest `/tmp` exhausted the
host volume because the VM disk uses that same physical backing. The failed
copy was not a second independent storage budget. Approved removal of only
rebuildable cache files and an ubuntu24 stop/start released deleted files held
open by the VZ process; source, patches, and evidence were preserved. Reboot
cleared the former `/tmp` native builds. The same frozen v15 source/configuration
was rebuilt under `/home/adamh.guest/.local/share/soac/builds/strict-opt-v15-01a02587`
and selected only after **262 native tests**, **37 CPython files / 3,849 cases**,
and ordinary reentrancy controls passed again. New executable SHA-256 is
`219434ef08a1b462e419812fbda8ef2dccf386fe8b490905aea180b3c73a42f8`;
libpython is `5bfddb0910e24f38b8cae60768f8fd7954844b1fb29c55f1c61f85e98c7140e6`.
The old receipt is historical, not evidence for the new executable. Current
receipt: `work/logs/strict-cpython-v15-persistent-recovery-ready.json`.

Selected-build persistence is now enforced before writing selection state:
external system-temporary paths and symlink escapes are rejected; a previous
selection remains byte-identical on failure. The existing environment tooling
family passes **34 tests** on the recovered guest interpreter. Its first run
used a shared-filesystem fixture incompatible with the existing case-sensitive
build preflight; the preserved failed run is not counted as a code failure.

Normal 0020/schema-5 checker
`05644b448f67b0d78dd20d1efcd994dd3d7d092c6dfcc743d3d7d631e1fe1b1d`
passes **34 actual CLI tests in 85.36 seconds**, including surrogate-source
admission, and **27 wrapper/toolchain tests**. The exporter fingerprint is
`d7578fc95bc39d7660857a31c95440d2d52eaa4dcaa9dfb8966ae475c8822106`.
Its lock refresh now uses `cargo update --workspace`: the only package addition
is the local `soac_source`, with every external locked version retained.
An initial `generate-lockfile` refresh would have upgraded three unrelated
packages; that attempt was restored before compilation and its diff retained.
The first CLI attempt omitted the selected interpreter environment and failed
setup; the complete corrected run is the gate above. Receipt:
`work/strict-ty-0020-v15-ready.json`.

The same normal binary and exporter fingerprint are revalidated against the
final selected v17 native generation after patches 0030/0031. All **34 CLI tests
pass in 87.13 seconds**, with byte-identical before/after checker, prepared
source generation, pin, selection, loaded-library and venv snapshots. The normal
wrapper reused its Cargo build in 0.51 seconds; complete source verification
and the help invocation took 41.16 seconds. The new receipt is
`work/strict-ty-0020-v17-ready.json`, paired with native generation
`a17ff7c541f2a2f3be921d6658d7eef812aed4263c15869e9c871360199342ba`,
executable `0d6116ee...82427`, and library `3d77bb16...cc90e`.
Native identity is separate from the checker source fingerprint: unchanged
checker bytes do not make prior-environment publications current. This gate
executes no strict initializer and stages no extension; fresh authenticated
runtime and benchmark publications use the new environment identity.

### Compiler cache and handled-context inlining

A focused before-test proved that build identity omitted `soac_source` and
several earlier compiler stages. Identity now covers the crate source tree;
all **four build-support tests** pass, including semantic edits to source
validation, core IR, optimization, typed IR, and the driver.

Structured production inlining exposed two independent failures: synthetic
blocks dropped the caller's nested handled-region prefix, and assigning a
callee result directly to an existing caller local could finalize the old
value under the callee's handler. Cloned callee blocks now compose the caller
prefix; synthetic guard/fallback/cleanup/continuation blocks retain the caller
context. A handler-bearing callee returns through a fresh temporary and the
caller assignment occurs after leaving its handlers. The two recorded red
regressions now pass with dynamic-context and ordinary-call-in-generator
positive controls (**three focused tests**). Generator resumes retain their
real capsule-owned handled-item boundary until an explicit separate activation
can be represented; absence of lexical handlers is not a C-API mutation proof.
Actual profile/apply/verify finalizer and exception-observer replays remain
pending the coherent extension build. Evidence:
`work/logs/strict-v15-inline-handled-context-before.log`,
`work/logs/strict-v15-inline-result-handoff-before.log`, and
`work/logs/strict-v15-inline-result-handoff-after.log`.

The full lowerer gate now passes **418 tests** and the optimizer gate **227**.
The handled-state native unit family passes **five tests**, and focused
deoptimization bare-raise/activation-ABI checks pass. Two test-harness errors
were corrected rather than changing production exception semantics: public
`PyErr_SetHandledException` retains a borrowed input, and the new deopt ABI has
an eighth explicit activation argument. An early failed assertion poisoned the
shared test mutex and caused follow-on errors; the first unpoisoned JIT run was
**696 passed / 16 failed**. Ten failures were old generator-erasure expectations;
their unchanged source snippets now assert retained activation, factory, resume,
and public storage identities. Six exposed a real handler-entry error edge that
omitted live SSA operands. A minimal real Cranelift verifier regression
reproduced that defect before repair; handler entry now uses the same planned
failure-cleanup transport as other throwing operations. The focused verifier
test and all ten retained-activation tests pass, followed by **713/713 JIT tests
in 3.65 seconds** (`work/logs/strict-v15-jit-final-gate.log`).

The coherent debug extension built in 34.41 seconds and was actually imported
against the persistent v15 library and normal 0020 checker. Its SHA-256 is
`8f0ba7d506cbffee21f4395b820df3eee353674a93db9c6cee8c42b29cb25f48`;
`work/strict-v15-coherent-extension-inputs.json` records the matching native,
loaded-library, checker, and Python-support identities. The frozen broad review
completed with **36 pytest passes and 14 aggregate failures in 1,378.15
seconds**; per-case outcomes are retained, rather than treating the aggregate
count as independent runtime bugs. One ignored broad-case review harness had
not wrapped its original assertion blocks in a declared validator; those
`module`-name failures are harness failures, not passing or failing behavior.
Its corrected retained-publication replay is pending.

The next coherent extension, actually imported as
`023028063f4c6750fc29cbbfbf690b4de134864294dc9c0c900afa27f39ac0ac`,
builds in 30.95 seconds after **420 lowerer and 716 JIT tests** pass.
`work/strict-v15-replay-extension-ready.json` records unchanged native,
loaded-library, checker and Python-support inputs; only the extension changed.
Original global/native-name and declaration-only cases now pass in both
entries, as do the original wrong-arity checks and the new 17-shape native
binder parity cases in both entries. All six user-annotation replay cases,
both retained fake-globals cases, both annotation-only cases, and eight
compiler-provider controls pass at their recorded fixed-input checkpoints.
The 23-case capture/binder gate is **17 passed / 6 failed**: the remaining
failures are closed-wrapper private-cell retention and original generator
expression code identity, not the repaired verifier or binder errors.

The docstring/reload/helper/operand gate is **47 passed / 8 failed in 149.27
seconds** with byte-identical runtime and validator inputs. It proves module
docstrings visible before body callbacks, permanent reload rejection without
reexecution, exact static/class method witness identity, and four suspended
operand controls. The eight failures are native-parity mismatches in four
ordinary-function assignment shapes, in both entries. Genuine original
`iter_refcount_behavior` also retains three objects until function return
instead of releasing them before its post-handler count. Evidence is in
`work/strict-v15-operand-lifetime-and-doc-replay-before/` and its log.

Assignment producers now carry an explicit operand lifetime and acquisition
sequence. A resolved `bb_prepared` pass materializes nullable quiet-delete
unwind blocks before source handlers, preserving original exception transport
and old handled state. It uses continuation liveness rather than a temporary
name prefix, and supports preserved generator storage through the same IR.
The minimal structural before-test fails for the missing cleanup edge and
passes after the repair (one test, 0.02 seconds). The widened lowerer suite
passes **423 tests in 0.21 seconds**; the combined Boolean-deopt/operand change
passes **717 JIT tests in 4.11 seconds**. The actually imported combined debug
extension is
`c8620129aae0647c819f4b8da9edee4faf1bb0cb6fa1592e422588df825c88ed`,
built in 33.54 seconds with byte-identical compiler inputs before and after
the build. `work/strict-v15-operand-bool-extension-ready.json` records its
actual native, library, checker, and Python-support identities.

The unchanged twelve-case operand test improves from **4 passed / 8 failed**
to **11 passed / 1 failed in 116.34 seconds**, including all four suspension
controls. Partial attribute-target assignment still retains its second
unpacked object until return in compiled execution; the entry interpreter now
matches native cleanup. A read-only retained-publication probe finds no Python
container holding that remaining object, and it is released on function exit.
This is a remaining JIT lifetime defect, not a passing cleanup claim.
Evidence: `work/strict-v15-assignment-operand-runtime-after/` and
`work/strict-v15-partial-operand-root-diagnostic/`. The same combined runtime
also passes both formerly failing nested-handler payload-collection cases;
None-valued completion context and cancellation-cycle tests remain unresolved.
BlockPy cache version 28 rejects older layouts and cleanup plans. No cleanup
throughput improvement is claimed.

A native watchpoint on that last failure records three references before the
setter: the input list, the unpack tuple, and the materialized element. The
first two are released on the failing edge, but the last reference survives
until compiled function exit. The reproducing structured test must pass
`Some(empty SpecializationProfile)` through production typed rewriting;
`None` skips linearization and did not reproduce the real operation shape.

Shared typed MAY/MUST entry facts now keep explicitly marked expression
operands only while live in their successor or explicitly passed as block
arguments. The handler therefore materializes a dead replacement as unbound
instead of transporting it as a frame root. Per-operation failure cleanup
unwinds the acquired value, while ordinary frame locals and hidden typed-plan
inputs keep their existing obligations. The genuine production-pipeline test
goes from **one failed test** to **one pass in 0.04 seconds**, and all **15
ownership-effect tests** pass, including widened hidden-input controls.
`work/strict-v15-typed-operand-plan-focused-after.json` records scoped
formatting and byte-identical compiler inputs during each check. Full runtime
afterproof is pending the next extension checkpoint.

The immutable `c8620129…` maintained delimiter/admission matrix completes
**898 passed, 19 expected failures, and 10 failed in 3,439.37 seconds** with
byte-identical runtime and validator inputs. The failures are five behaviors
in both entries: logical-frame `dir`, generator-expression inherited capture,
generator-expression code transplantation, the generator-expression part of
named-expression scope, and cancellation refcycles. No failed case was
removed or reclassified by this run. Evidence:
`work/strict-v15-maintained-delimiter-all/`.

Full structural gates after typed-operand and deopt-capture repairs expose
two changed lifetime assumptions: the JIT test expected a dead factory
operand to be forwarded into a handler, and the iterator optimizer expected
the exception matcher to be its immediate successor. The former assertion
now requires an unbound handler entry. Inspection of the latter exposes a
real producer bug: augmented assignment never deleted its old-value, receiver,
or key temporaries on success, so a later iteration could unwind a still-live
old operand before matching `StopIteration`. Skipping that cleanup would
silently change finalizer behavior and is not an acceptable optimization.

A genuine ordinary/strict fixture for name, attribute, and subscript `+=`
targets fails **all six entry/compiled cases in 68.94 seconds**, with unchanged
inputs. Replacement values and temporary receivers survive until function
return instead of their native operation boundary. The repair materializes
the operator result, deletes the old-value operand before the target store,
and releases key/receiver/result in native stack order. Producer metadata is
named `unwind_order`, not acquisition order: the result reserves its position
below the target operands before evaluation, so a failing setter also releases
key and receiver before result. Six producer/bypass-lowering tests and the
resolved target-error cleanup test pass; the full lowerer passes **427 tests
in 0.27 seconds**. All **227 optimizer tests pass in 0.16 seconds** after a
27.00-second build, including the original iterator-backedge specialization
regression. No optimizer workaround or cleanup bypass was added. The unchanged
actual runtime replay remains pending the next native/extension epoch. Evidence:
`work/strict-v15-augmented-operands-before/` and
`work/logs/strict-v15-iterator-backedge-plan-diagnostic.log`,
`work/logs/strict-v16-generator-return-full-lowerer.log`, and
`work/logs/strict-v16-augassign-full-optimizer.log`.

An unchanged authenticated `dir_filters` replay proves another concrete
compatibility defect: both entries return the ordinary driver's namespace
names, not the function's `junk` local or an explicit unsupported-operation
error. `work/strict-v15-dir-actual-frame-before/` records two failures in
12.96 seconds with byte-identical inputs. The native explicit-context call
dispatcher recognizes `locals`/`vars` but omits canonical `dir`; its repair
must preserve object-argument calls and ordinary argument errors.

The isolated `0030` candidate adds canonical zero-argument `dir` to that
dispatcher. It sorts the supplied namespace's keys, pins the mapping while
its `keys()` result is consumed, and explicitly rejects missing function-local
context. Object calls, aliases, Python replacements, callback exceptions, and
native argument errors retain their ordinary paths. The selected-v15 native
family has **three failures / seven passes** before the repair; the candidate
passes **all ten**, including a reentrant mapping-lifetime case, plus **264
native tests**, **37 CPython files / 3,849 cases**, and the separate **147-case
builtin suite**. Ordinary `dir` controls match selected v15 in both normal and
development modes. The independent 5,541-file replay changes only the builtin
dispatcher and its header comment; generated cases and all ABI probes remain
unchanged. This is an **unselected private debug build**, not a selected-v16 or
transformed-runtime afterproof. The four maintained strict-context tests still
fail on selected v15 as expected. Evidence:
`work/cpython-dir-context-candidate/final-gate.json` and
`work/strict-v15-dir-context-tracked-before/`. The first evidence formatter
rejected CPython's successful `441 ms` duration because it assumed seconds;
that log is preserved, and the final receipt parses the retained units and
rechecks the actual source/native/test snapshots without substituting results.

The shared-source optimized v16 build subsequently passes **264 native tests**,
**37 CPython files / 3,849 cases**, and **147 builtin cases**, but remains
unselected after the fixed 97-benchmark dependency preflight prepares only
**95 of 97** environments. Both SQLAlchemy cases fail while compiling greenlet:
the public `PySoacTypeConstructionSpec` field named `namespace` is a C++ reserved
word. This is our header defect, not a dependency-version exception; no package
pin, benchmark version, or denominator is changed. The focused native-header
test reproduces **one failure in 0.100 seconds** with the same compiler error.
Patch `0031` renames that field to `namespace_dict` and updates its native,
Rust, and ctypes consumers without changing field order, layout, or ABI version.
Its independent 5,541-file replay and C++ header-only control pass. The coherent
shared-source optimized v17 PGO/LTO build completes in **435.50 seconds** with
the same interpreter flags and workload. The actual public-header regression
passes, followed by **265 native tests**, **3,849 CPython cases across 37 files**
(46 skips), and **147 builtin cases** (12 skips). Normal and development-allocator
`dir` controls match, the physical ABI is unchanged, and source/runtime/test
fingerprints and the complete independent replay remain equal. v17 is selected
at **14:38 PDT on August 22, 2026** in persistent guest storage; the venv starts
without `LD_LIBRARY_PATH` and loads its exact library. v15 and the unselected
v16 binary, build records, and prior gates are preserved. No extension is staged
by this native handoff. The two SQLAlchemy dependency retries and fresh runtime
and performance gates remain pending, not waived.
Evidence: `work/pyperformance/dependency-retry-v15/sqlalchemy_declarative.log`,
`work/logs/strict-v16-public-cxx-header-before.log`, and
`work/cpython-public-cxx-candidate/source-ready.json`, and
`work/logs/strict-cpython-v17-ready.json`. The selected generation is
`a17ff7c541f2a2f3be921d6658d7eef812aed4263c15869e9c871360199342ba`;
the executable and libpython hashes begin `0d6116ee` and `3d77bb16` respectively.
The first dependency
preparation redirected Python output but missed inherited installer file
descriptors; `work/retry_strict_suite_dependencies.py` explicitly captures those
descriptors and retains the real compiler diagnostics.

The build workflow now syntax-checks public `Python.h` after configure creates
`pyconfig.h`, before compilation or PGO. An extra make target uses the actual
configured `$(CXX)` rather than guessing a compiler. Failure preserves the saved
selection and stops before publishing new provenance. The focused build-order
test first fails because no check runs; the updated tooling family then passes
**39 tests** in 0.12 seconds, including configured-compiler selection and paths
containing spaces. The new target also passes against v17's real Makefile and
`g++` in 0.147 seconds. This hook was added **after** the v17 build receipt:
that actual smoke test is postbuild evidence, not a claim that the historical
v17 PGO run used the new early hook. Receipts are
`work/logs/strict-cpython-v17-cxx-preflight-{before-02,after-03}.json` and
`work/logs/strict-cpython-v17-configured-cxx-preflight-actual.json`.
The first tooling attempt used case-insensitive shared storage for its mock
build directory and correctly hit the existing storage precondition; the real
before/after tests use a guest-local directory instead. That setup-only failure
is retained separately from the genuine missing-check regression.

The maintained broad-import cohort also exposes two entry-only defects:
`locals_recent_assignment` returns caller globals instead of its explicit
unsupported-function-locals error, and `deleted_super_first_arg` raises
`UnboundLocalError` instead of `RuntimeError: super(): arg[0] deleted`.
`soac.runtime` deliberately shares one rejection callable between several
frame-sensitive names, so Python pointer equality cannot identify `globals`.
Entry/deopt dispatch now consumes the compiler's immutable `RuntimeName`
projection alongside the exact module-constant indices; dynamic aliases do not
acquire that role. The compiler-inserted implicit-super local operand uses the
existing deleted-first-argument error without changing ordinary source loads.
Two production-path kernel tests go from **two failures** to **two passes in
0.05 seconds**, including native aliases, shared-helper rejection, actual deopt
constant roles, valid super, and an ordinary deleted-local control. Typechecking
also passes. The unchanged maintained Python cases then pass in both entry
modes on the actual v17/f1bb extension: **four passing outcomes**, with exact
runtime/checker/support snapshots before and after. Evidence:
`work/logs/strict-v15-entry-frame-call-provenance-{before,after-02-focused}.log`.

The same v17 contextual-call cohort has **eight passes and two failures** in
99.82 seconds. Original function `dir` safeguards and late-bound method calls
pass, but a new module-level builtin alias fails before entering its class:
name binding supplied a class mapping only, leaving module calls with the same
NULL namespace used for unmaterialized function locals. Two production-path
JIT/entry tests reproduce that missing module context. The repair introduces
the public core `FrameNamespace::{ModuleGlobals, Mapping}` operation payload:
the former uses the defining environment, the latter retains its explicit
mapping operand and cleanup, and ordinary functions keep `None`. Every call
selects its own context rather than inheriting a containing operation's mapping.
Module and class calls stay on the existing public boundary; no inline or
direct-call plan may silently replace their source environment. Lowering's
scope/serialization test and typed planning's preservation/stale-plan rejection
test pass. BlockPy cache 30 excludes the old ambiguous representation. Joint
JIT and actual staged afterproof remain pending; native v17 is unchanged.
Evidence: `work/strict-v17-context-after-f1bb7a7d/result.json`,
`work/logs/strict-v17-module-frame-namespace-before-02.log`, and
`work/logs/strict-v17-frame-namespace-{lowerer,opt}-after.json`.

Publication and replay must use the same recipe environment. Two attempted
retained replays used direct `just --command` instead of `_pytest-run`'s
`LD_LIBRARY_PATH` prefix and were correctly rejected before admission. Fresh
real publications or the exact original recipe environment fixed the workflow;
no signed environment digest was rewritten or authentication check disabled.

The legacy review accounts for all **298 delimiter cases**. The initial 169
mapped cases and 119 individually reviewed candidates are now **288 explicit
strict/admission/interoperability routes**, plus invalid syntax and nine
existing documented-limitation cases. A structural inventory finds no missing
or duplicate route. This is maintained enrollment, not a claim that the full
288-case runtime matrix passes. Unknown future cases now fail with a review
diagnostic before an unauthenticated transformed attempt. Reviewed broad-import fixtures
add 35 actual publications and 12 individually confirmed strict rejections.
The framework/annotation review publishes 20 of 24 original sources; four
originals have confirmed checker errors and retain ordinary controls. These
are offline results, not runtime passes. An earlier `typing_import`
classification mistook a co-printed warning for a blocking error; replaying the
identical project with both archived 0019 and current 0020 proves both accept
it with the same signed warning. It remains an admitted compatibility case.

| Requirement | Current evidence |
| --- | --- |
| Aligned strict policy and compatibility contract | Aligned target documentation; authenticated runtime integration in progress |
| Genuine conservative Python 3.15 `ty` exporter and strict diagnostics | Unchanged normal 0020/schema 5 passes all 34 actual CLI tests on selected v17; 27 toolchain tests passed at the prior recorded epoch. Source-safe fork passes 142 project and 485 semantic tests. Actual dataclass nominal 16-outcome matrix passes at its recorded epoch; new coherent runtime replay pending |
| Authenticated, versioned, incremental artifacts and runtime loader | 58 shared-contract tests; prior checkpoint passes 31 offline CLI tests. Actual nominal/annotation admission, two-venv checks, and mutation-after-constructor rejection pass; constructor-only paired evidence recorded separately |
| Explicit nonforgeable pre-callback type construction | All eight genuine class cases pass on v4, including pre-init_subclass protection and repeated-definition ownership |
| Permanent Python, bytecode, supported C API mutation barriers | Shared-source PGO/LTO v17 selected with 265 native passes, a real C++ header compile, and 37 CPython files / 3,849 cases. Actual source/generated CREATE invocation checks pass at their recorded epochs; new preserving-C generator-expression probes distinguish ordinary CPython failures from strict admission defects |
| Checked public entries, returns, fields, and sound check elimination | Earlier function/field/nominal gates pass; v9 genuine checked-call tests pass both actual entry modes, with individual apply/verify elimination and unchanged mandatory checks/fallbacks |
| Stable physical instance layouts and inherited prefixes | Fixed-prefix and actual native-slot profile/apply/verify paths pass with structured storage-kind evidence. Native-linked owner-address reuse regression and actual per-location hybrid after-tests pass |
| Validated virtual/direct dispatch with actual callee environments | v10 actual fixed-body/temporary-receiver four outcomes and free-function two outcomes pass; native hit counters, structured emitted sites, imported/forward/factory targets and ordinary fallback verified |
| Authenticated ordinary/slotted dataclass adoption and framework fallback | Real Pydantic, Django, SQLAlchemy comparisons pass both modes. Actual dataclass dictionary/slots, nominal fields, pickle, preserving CREATE, hybrid and failed-application cases pass. The retained fb968 ten-outcome replay closes compiled replacement lifetimes and both identified admission/routing defects |
| Full acceptance matrix and structured optimization decisions | Focused authenticated families and structured plans are covered; the complete combined runtime matrix and full gate remain pending |
| `just test-all` | Early v6 run failed; helper selection passes 68 tests. Maintained 927-case delimiter/admission gate: 898 pass, 19 expected, 10 fail. Completion/capture/operand checkpoint: lowerer 427 and optimizer 227 pass; v17 combined JIT passes all 730, including the repaired temporary-lifetime assertion and deopt failure ownership. Two later cancellation transport tests are intentionally one red/one green until the source-lifetime repair. Actual runtime replays, remaining authenticated fixture migrations, and the full gate remain pending |
| Three-round full-suite strict/stock and prior-strict evidence | Preparation/provenance/recipe tests, actual sealed profile/apply worker smoke, and all 97 driver harness projections pass. Real v17 dependency preparation succeeds for all 97. After preserving upstream project metadata and adding the disclosed fixed policy, actual offline analysis publishes 53 and rejects 44. The complete set remains fixed; no performance rounds or full-suite speedup yet |
| Reconciled source identity, finalized strategy/performance log, main integration | Shared source verified; final evidence/integration pending |

### Real fixed-suite preparation on v17

The first complete dependency-plus-offline preflight uses the actual selected
PGO/LTO v17 executable, its loaded library, and the unchanged normal 0020
checker. All **97 of 97** dependency environments prepare successfully,
including a genuine C++ build of the unchanged `greenlet==3.2.4` pin. The two
earlier SQLAlchemy dependency failures are therefore closed at the real
installer boundary, not merely by a header-only unit test.

All **97 of 97** offline preparations then fail before invoking the checker:
the installed upstream driver directories contain `pyproject.toml`, and the
source overlay correctly refuses to overwrite it. The previous 97-driver
success covered only the measurement-harness projection; it did not exercise
the complete overlay/publication path. The fixed set is unchanged. A tested,
disclosed addition of strict policy must preserve the original metadata and
reject conflicts. No failed driver is removed and no ordinary-SOAC lane is
substituted.

This diagnostic run took **286.00 seconds**; native, checker, and preparation
script inputs stayed byte-identical. Its publications are not reused by timed
workers, because subsequent dependency preparation can change a shared venv.
There are no elapsed-workload results or JIT-coverage claims from this run.
Evidence: `work/pyperformance/strict-v17-suite-offline-preflight/result.json`
and its per-driver dependency logs. Capturing inherited subprocess file
descriptors, rather than only Python output streams, retains the actual C++
compiler/installer diagnostics.

The preserving policy merge then passes **42 focused preparation tests**. It
keeps the original TOML bytes and values, appends only the disclosed fixed
strict policy, and rejects conflicting existing policy. Repeating the complete
real preflight takes **1,208.85 seconds** with unchanged native, checker, and
preparation inputs: all **97 dependency environments** prepare, **53 drivers
receive authenticated publications**, and **44 are rejected** by actual
analysis. `chaos` is among the accepted drivers. The fixed set is not narrowed.
Evidence: `work/pyperformance/strict-v17-suite-offline-after-policy/result.json`
and its per-driver checker and dependency logs. These remain offline
preparation results, not timings, native execution, or transformed coverage.

Representative diagnostics distinguish source incompatibility from compiler
bugs: `2to3` imports `lib2to3`, absent from the selected CPython; both argparse
entrypoints declare `ArgumentParser` returns but have no return statements;
the asynchronous-tree workload has an unresolved nullable return at a consumer.
These findings do not justify changing workload annotations or suppressing
diagnostics. Stock execution and the remaining rejection families still need
classification. The production comparison also needs to preserve per-driver
failures and finish all requested rounds: upstream pyperformance continues
after a driver's rejection, but the recipe currently stops after the first
partially failed profile pass. An incomplete run must retain its fixed plan
and report no full-suite geometric mean, rather than claim success from an
available subset or a single pair of an intended three-round comparison.

## Verdict and next action

- The early full gate also exposed validation-workflow defects, not evidence
  of 142 independent runtime bugs. Its phase runner re-enabled shell `errexit`
  in the outer status collector and stopped after the first failing phase;
  isolating those shell options passes a behavioral test of five phase-status
  scenarios. Separately, 300 legacy delimiter cases define validators that
  their dispatcher never calls, and their ordinary source has no strict startup
  authority. Those legacy passes are not strict compatibility evidence. The
  exact-once validator and real-failure reporting repairs pass 14 harness
  regressions; individual authenticated migration is in progress. No blanket ordinary-SOAC admission
  or text-based unsupported-failure suppression is an acceptable substitute.
  Evidence: `work/logs/strict-test-all-v6-early.log` and
  `work/logs/test-all-phase-regression-after.log`.
- Verdict: still in progress; the complete contract and performance target remain
  unproven.
- Transferable lesson: distinguish semantic predictions, checked artifact
  proposals, actual installed runtime policies, and sealed optimization facts at
  every boundary. A same-named checkout or interpreter version is not proof of
  identical native source.
- Next: finish fixed-artifact runtime replays on the combined v17 extension,
  retire compiler-owned exception transports at their semantic lifetime,
  close the preserving-C generator-expression and module/class `dir` cases,
  complete the authenticated fixture migrations, and run the full gate. Then
  finish classification of the 44 offline rejections and the fixed benchmark
  protocol without dropping missing results.

## Checked unbound calls with omitted defaults: checked-call extension

A genuine normal0020/persistent-v15/extension8f0ba7 profile/apply test of
`callee(value, increment=5)` called as `callee(37)` returns 42 through the
actual checked-native public entries, but selects no direct CLIF call edge.
The immutable diagnostic is
`work/strict-v15-default-edge-diagnostic/result.json`; the original failing
positive validator remains in
`tests/test_regression_direct_call_defaults.py`. Emitted inventory contains
the 436-byte callee body, 112-byte default adapter, and 896-byte caller body;
the apply rewrite reports no inlining. Neither successful behavior nor these
compiled bodies proves that a direct edge was selected.

Before this extension, the checked-body planner accepted sealed method regions,
not unbound source-function call sites. The strict-module exclusion from unchecked
target/inline plans must remain: a plain source or profile target is not a
dominating argument/return boundary proof.

The candidate implementation reuses that checked planner, raw captured-call
argument path, normal current-default binder, prepared activation, and guarded
native-body region. An exact signed unbound-call site may nominate a matching
source/native-ABI target. The supplied argument count is distinct from the full
bound body arity; inserted defaults have no reusable caller proof and still
receive their required checks. Only the actual captured function's checked
entry and activation authorize execution. A target mismatch retains the same
activation's environment or the same captured callable's normal public call,
without replaying expressions. No unchecked default adapter or new Python
argument container is authorized.

The source-selected default regression first failed with zero planned checked
bodies, then passed after the planner change. The coupled raw ABI/emitter source
subsequently type-checked: preparation takes both supplied and bound arities,
zero-argument bodies allocate a non-null zero-capacity output buffer, and the
existing finish path releases every binder-owned explicit/default value. Body
parameters still borrow these references; this is not a workaround for the
separate deoptimization ownership defect. The selected region is atomic through
linearization, with the actual callee independently retained across argument
callbacks. Fixed-body emission remains guarded by the actual activation's body
pointer; misses do not repeat lookup or argument effects.

A fresh genuine baseline on immutable extension `c8620129...c88ed`, support
`289cd414...edb914`, normal0020 and persistent v15 completed 2 passed / 1 expected
optimization failure in 65.88 s. Both entry modes pass pre-seal current-default
mutation, required argument/default/return failures, same-code factory defaults,
current closure-cell target changes, ordinary replacement fallback, and sealed
default mutation rejection. Apply stops at the unchanged positive
`direct_body_calls` delta assertion. Runtime and test-input snapshots are
byte-identical in `work/strict-v15-checked-free-default-before/result.json`;
these successes are before-controls, not candidate direct-path verification.
The original three default/arity/argument-error validators remain unchanged.

The expanded production-planner family passes five tests in 0.04 s, and
the guarded fixed-body emitter kernel passes in 0.05 s for zero, one, and two
bound arguments. This covers exact signed-site selection, full-arity default
projections, malformed/dynamic target rejection, atomic argument regions, and
the guarded fixed versus same-activation indirect code shape. Logs:
`work/logs/strict-v15-checked-unbound-focused.log` and
`work/logs/strict-v15-checked-fixed-body-focused.log`.

Two additional genuine before-controls pass in 61.61 s on the same frozen
runtime. Argument callbacks rebind the captured callee while the old function
remains alive through its call; argument and default-binding failures release
the last callee/default roots without replay. A C-API public-vectorcall
replacement invokes the replacement exactly once. The original ordinary
controls pass in the same subprocess. Evidence and identical snapshots:
`work/strict-v15-checked-free-callback-before/result.json`. These remain
before-controls, not evidence that the new direct path passed.

The rebuilt v17 checkpoint now passes all fourteen selected default/call
outcomes, including the unchanged positive direct-body counter assertion,
argument/default/return errors, current-target and captured-callee cleanup,
public-vectorcall replacement, and sealed-mutation rejection. One outcome is
the genuine checker-negative/ordinary-control test, not a runtime call test.
The actual extension is
`f1bb7a7d52d5bc0573967d82d72ba7ef86dad4416bffbc07b3eaf60b876fe562`;
normal0020, selected v17, loaded libpython and support289cd414 remain fixed.
The combined 48-outcome gate completes in 402.68 seconds with byte-identical
runtime and selected-test snapshots in `work/strict-v17-ty-owned-after/`.
Its two aggregate failures are separately diagnosed validator-builtins issues,
not default-call failures; their corrected retained replay is recorded below.

Status: source, structured selection/emission, and the bounded actual direct-path
afterproof pass. Stock and previous-SOAC timing, candidate code growth, and
pyperformance coverage are pending. No throughput improvement is claimed.

## Generator-expression original-code exposure

The unchanged original-code validator fails in both runtime modes after its
named-generator identity and interleaving checks: a generator expression's
`gi_code` is a freshly manufactured runtime template rather than the actual
original code object in its source function's `co_consts`. Evidence remains
under `work/pytest/strict-v15-compatibility-capture-binder-after/`.
The maintained delimiter matrix independently reaches that ordinary template
when it copies the exposed code into an ownerless function; this must not
become callable strict authority.

The source candidate adds the explicitly re-exported
`soac_core::block_py::GeneratorExpressionCode` projection to callable scope and
the authenticated template shape. The original parser supplies the complete
expression and first-iterable byte ranges. Native matching requires the exact
privately compiled source tree, one unambiguous `<genexpr>` with the expected
generator kind and `.0` ABI, an exact first nonempty iterable position, and
remaining positions within the expression. Same-line and nested expressions
cannot be selected by name or iteration order. Missing or ambiguous mapping
never falls back to a code name.

A separate GC-traversed code-exposure map supplies only public `gi_code/ag_code`
and native names. It grants no SourceFunction admission and does not replace
the helper's execution code, original closure ABI, or required rooted compiler
creation. Copying the exposed strict code still has no native function owner.
Cache generation 29 is coordinated with explicit generator completion metadata.
The six-expression parser/lowering regression passes. The cache roundtrip
and runtime-ID remapping test preserves the code projection and explicit
GeneratorReturn structure; the full native-independent driver suite passes
11 tests in `work/logs/strict-v17-cache29-driver.log`. Native-selector and new
same-line/nested/multiline/async identity regressions are written; native-linked
and actual runtime afterproof remain pending the next coherent
selected-native/checker checkpoint.

The delimiter matrix also found a distinct walrus capture-order defect:
`a = 1; gen = (b := a + i for i in range(2))` reads empty `b` as `a`.
The test-first structured regression proves public freevars `[b, a]` versus
resume freevars `[a, b]`, in
`work/logs/strict-v16-generator-capture-order-before.log`.
Assignment discovery had been reused as public closure ordering. The producer
now sorts only freevars by logical name, matching ordinary name binding;
preserved-state indices do not move. The regression passes for walrus and
named generators in both ordinary and strict lowering, and an existing layout
test now interleaves reverse-ordered captures with unchanged physical state
slots. The initial after-test overconstrained a nonlocal store to Closure
rather than the legitimate CapturedSource variant; its correction still checks
the exact public-cell index, and the original wrong-order assertion is unchanged.
The corrected focused receipt is
`work/strict-v16-genexpr-lowering-after2/result.json`, with identical source
snapshots and all nine selected outcomes passing. The full lowerer suite
subsequently passes 427 tests after the completion owner updates an obsolete
synthetic-raise test to explicit GeneratorReturn structure. These are
native-independent compiler tests, not actual runtime afterproof.

The selected-v17 native selector passes in the combined **730-test JIT gate**.
The fresh authenticated f1bb runtime then passes all four original-code
outcomes, including the unchanged original validator and the added same-line,
nested, multiline, async, and ownerless-copy checks in both entries. The three
unchanged delimiter regressions also pass all three ordinary controls and six
authenticated outcomes: inherited capture order, named-expression scope, and
generator-code transplantation. Evidence is the same immutable
`work/strict-v17-ty-owned-after/` receipt. Its retained harness imports
`strict_pyperformance_sources.py` only for strict opt-in; that file was held
unchanged throughout, with explicit supplemental before/after hashes rather
than a retroactively enlarged primary snapshot.

A separate preserving-C CREATE observer subsequently proves a pre-initialization
hole in the compiler helper, not the exposed original code. Captured strict
helpers have no closure or owner at CREATE and early native invocation crashes
both modes at COPY_FREE_VARS. Raw ordinary CPython exhibits the same C-API
initialization hazard, recorded independently. Zero-capture helpers instead
reach an inert template NameError, which still fails the required explicit
pre-entry denial. Exact strict-only evidence and identical snapshots are in
`work/strict-v17-genexpr-create-captured-only/`.

The bounded repair reuses the existing denial-only canonical code clone for
the authenticated generator-expression projection. It checks the current rooted
template and the separate code-exposure entry before CREATE, never selecting
authority from a function name/kind or inventing SourceFunction provenance.
The helper's source ID stays zero; public original `gi_code/ag_code` remains
unchanged. The native guard kernel now includes zero/captured generator and
async-generator code, and the tracked actual observer regression uses the shared
preserving C shim. Compilation and actual afterproof of this additional guard
remain pending; the earlier code-identity success does not establish its safety.

## Closed private-capture wrapper lifetime checkpoint

The immutable v15/0020/extension023 replay completed 17/23 actual outcomes in
224.83 s. Its four generator/coroutine lifetime failures were not failed
capture admission: classes sealed, required field predicates held, and their
instances/classes collected, but closed wrappers retained the actual source
function and its private cells. The other two failures are the separately
tracked original generator-expression code-identity mismatch. Both runtime and
test input snapshots were byte-identical in
`work/strict-v15-compatibility-capture-binder-after/result.json`.

A two-mode retained-publication diagnostic found only the wrapper as a
referrer to that source function, after native preserved-state edges had been
cleared. Releasing exactly `_resume_function` released its private target.
Production now drops that wrapper edge on terminal generator/async-generator
steps; coroutine wrappers use the generator path. It leaves source cells,
other live function references, other suspended frames, and public code
metadata intact. Release occurs after leaving the transport handler so
finalizers observe the surrounding caller's handled exception, and the
transport exception's original context is restored when rethrown.

The four original generator/coroutine validators pass unchanged in 60.91 s
after fresh genuine publication, with extension/native/checker unchanged and
byte-identical runtime/test snapshots. Exact evidence is
`work/strict-v15-closed-private-capture-after/result.json`, using support
aggregate `42deb0379265651b79fe35bb94ac8239d4c4bbf3953fca126c27801ac7a63429`.
A subsequent comment-only targeted lint explanation changes the aggregate to
`289cd4142bc113293b02d35c566b20d209e6d93509274c9d7884fbd56cedb914`;
the semantic AST is identical and the original receipt is not relabeled.
The distinction is recorded in
`work/strict-v15-terminal-owner-comment-only.json`.

All twelve additional genuine cases pass in 168.05 s on coordinated extension
`c8620129aae0647c819f4b8da9edee4faf1bb0cb6fa1592e422588df825c88ed`
and support289cd414: shared generator/coroutine/async-generator frames retain
independent ownership, required field checks remain enforced, and terminal
close/completion finalizers match native surrounding-handler behavior in both
entry modes. Runtime/test snapshots are byte-identical in
`work/strict-v15-terminal-owner-expanded-after/result.json`.
This is a correctness repair, not a throughput claim. Full-suite compatibility
and fixed stock/previous-SOAC performance measurements remain open.


## Authenticated module-precondition migration

All nineteen original source/ordinary-validator pairs are preserved in
`tests/fixtures/strict_module_precondition_cases.json`. Fourteen retain native
behavior through authenticated entries; only the five reviewed post-seal global
replacement routes (attribute, dictionary, function globals, exec, and C API)
expect `StrictMutationError` while proving the original binding and call result
remain intact. The same source without the strict future remains an ordinary,
unowned native control.

The first f1bb run passes all nineteen ordinary controls and seventeen strict
validators in each entry mode. Two limited-builtin cases fail before validation:
the shared validator harness copied the source module's deliberately restricted
`__builtins__` and could not execute its own imports. The declared validator now
uses ordinary builtins without modifying the analyzed module's captured mapping.
All thirty-eight unchanged strict validations and nineteen ordinary controls
then pass against the same genuine publication in **145.95 seconds**, with
byte-identical runtime/test snapshots:
`work/strict-v17-preconditions-validator-after/result.json`.

Only after that proof were the nineteen obsolete in-process wrappers removed.
The maintained helper/test ASTs and fixture source/validator hashes are unchanged;
the migrated file collects exactly twenty-one outcomes (nineteen ordinary and
two nineteen-case authenticated cohorts). Ordinary controls pass again after
removal. The extraction, original archive, and removal proof remain under
`work/strict-module-precondition-migration/`. No annotation, cast, diagnostic
suppression, or source-policy waiver was introduced.

## Actual deoptimization entry diagnostics

The fixed native GDB replay proves two real handoffs from compiled code into
`dp_jit_deopt_resume`; the original permanent counter assertion still reads
zero because no production pass creates its old preplanned counter definitions.
The diagnostic now belongs to the immutable compiled resume table. After
native compilation, each table registers its own append-only module-owned
atomic counter set, without moving published scalar storage or retaining
Python/capture/table owners. The cold helper increments only after validating
the table, ordinal, and incoming buffer. The obsolete hot emitter and
preplanned collector are removed, avoiding a duplicate path or double count.

The structured dump test proves separate recompilation IDs, source/function
identity, stable scalar storage, and retained counts after a table reference
is dropped. It passes in the **732-pass / one-failure** combined JIT run; the
failure is the separate contextual-call test counting compiler helper calls
alongside its one source call. All three instrumentation-mode tests pass.
Actual permanent-counter runtime evidence remains pending the next coherent
extension. Existing once-per-output-path flush behavior remains explicit;
these snapshots are not continuously updated telemetry or planning inputs.

## Escaping synchronous generator materialization

The unchanged closed-pipeline compatibility validator requires an escaping
named generator to be exact `types.GeneratorType`; the current strict wrapper
fails in both entry modes. The native source-code fallback remains forbidden:
executing original strict bytecode would bypass owned nested-definition and
class-construction admission. The correction therefore needs an exact native
generator with an explicitly owned, validated compiled resume record, not a
weaker type assertion or a source-flag exception.

The native protocol will keep the existing record-owned handled-exception item
as its sole activation and leave the ordinary generator's native item empty.
Every direct/specialized frame-push path must decline such a managed generator.
The materialized generator retains its exact original code; an eliminated
execution frame cannot be presented as a stale or empty successful `gi_frame`.
Live frame inspection must fail explicitly, while a truly closed frame is
absent. This uses the existing eliminated-frame boundary, not permission to
drop computed values, callbacks, or cleanup.

Eleven selected-native controls establish raw throw argument forwarding,
delegate-lookup and exception-constructor callback order and caller context,
nonterminal invalid throw handling, reentrancy, and CPython's `close()` return
value and created-generator throw behavior without body execution. The
maintained family is `tests/test_strict_generator_protocols.py`. Its first
genuine before-replay passes nine native controls and fails all eighteen strict
outcomes in **138.28 seconds**; the two added created-state controls pass while
all four strict outcomes fail in **57.09 seconds**. Both receipts have
byte-identical native/checker/extension and selected test inputs:
`work/strict-v17-generator-protocols-before/` and
`work/strict-v17-created-generator-protocols-before/`. The separate minimal
materialization control fails only the unchanged exact-native-type assertion
in both entries in **16.31 seconds**. These are concrete before failures, not
passing native materialization evidence. The planned cold delivery
discriminator is explicit state owned by the preserved activation: a delegate
error must enter its existing StopIteration handler without calling the
delegate twice, and a directly injected exception must skip repeated
delegation. Normal send/resume retains its existing four-argument ABI.

The isolated native debug candidate now passes **23 kernel tests** both
normally and with development checks, **288 native tests**, and **4,118 CPython
cases across 41 files** with 46 skips. The actual C probe checks the 40/32/16-byte
spec/input/result ABI and unchanged generator/coroutine frame offsets. Exact
binary/library/test identities and selected-v17 stability are recorded in
`work/cpython-managed-generator-candidate/gate-draft3-full.json`. This is private
kernel evidence only: the selected shared-source runtime remains v17, and the
Rust consumer, final shared-source build, and actual strict after-replay are
not yet validated.

Four more ordinary controls distinguish injected-exception context from a
source `raise`: a generator with an empty owned handler item must not acquire
the caller's context, while a delegate-raised error retains its existing
context. A corrected genuine factory replay passes eight outcomes and fails
four in **90.88 seconds**, with byte-identical inputs. Plain strict throw adds
the caller's `RuntimeError` incorrectly, and created throw executes the body;
the own-handler and delegate-error controls already pass. Evidence is
`work/strict-v17-generator-injection-factory-before/result.json`. An earlier
attempt hit the fixture's normal-entry assertion on a generator factory and
is not counted as semantic before-evidence. The planned injection helper must
use the actual owned item instead of ordinary raise's topmost-handler lookup.

## Preserve failed full-suite comparison evidence

The real v17 offline attempt prepared all 97 dependencies and published 53
strict driver bundles; 44 drivers retained blocking checker diagnostics. This
is publication evidence, not measurements. The previous comparison shell
stopped after the first failed profile, skipped apply and later rounds, and
could discard its summary. A one-pair directory could also be mislabeled as a
complete three-round request. Three focused before-tests reproduced those
reporting failures.

The production recipe now freezes the full driver set, round count,
alternating order, outputs, arguments, and baseline in `comparison-plan.json`.
Per-phase driver journals preserve preparation and worker failures and copy
fresh checker diagnostics before later phases overwrite them. Profile failures
do not suppress apply or later rounds. Incomplete results retain nonzero
status and have no full-suite mean or merged aggregate. Per-result diagnostics
require every requested pair plus matching driver, source, and interpreter
evidence; available apply seal, JIT, and size evidence remains explicitly
partial. Profile failure is still disclosed if a later apply succeeds. Partial
results are not an acceptance score.

The final structured/tooling gate passes **138 tests in 2.21 seconds**, with
one real analyzer/worker test deliberately deselected. The actual CLI and
orchestration use synthetic subprocess outcomes for those tests, not timing
thresholds. `work/strict-v17-comparison-runner-ready.json` preserves the exact
unchanged inputs and scoped lint/format evidence. New real fixed-suite stock,
previous-strict-SOAC when available, and candidate measurements remain pending;
no performance conclusion follows from this tooling gate.

## Pending payload and exception-forwarding lifetimes

Pending finally values have control-flow extents rather than last-read
lifetimes. The first retirement candidate failed its structured control by
clearing the old return before the finally body. On immutable f1bb/v17,
the maintained pending-return family passed four and failed six outcomes in
**83.11 seconds**. The added interleaved control failed both entries in
**46.99 seconds**: the inner pending value was destroyed under the caller,
not the source handler. Runtime and fixture snapshots remained unchanged.

The repair declares a primary `AbruptPayload` at a distinct finally entry and
carries its extent through `EnclosingAbruptPayload` roles. Cleanup evaluates
overriding operands first, trims existing handler records with an explicit
`Unwind` transition, and releases each owner in its proper enclosing state.
Unwind neither enters a fresh handler nor consumes the pending raised-scope
marker. Region identity survives retirement of its original operand and
supported C-API replacement of the actual handled exception.

Retirement keys physical `LocalLocation` and `PreservedLocation` owners
separately. Generator lowering supplies explicit yield-to-resume metadata,
including mandatory zero-parameter wrappers, so only preserved owners cross
suspension. The metadata is consumed before optimization. The mandatory
wrapper repair passed five focused tests and the full **436-test lowerer
suite**. Actual cancellation, handler/finally destruction, and suspended
C-API replacement afterproof still require the next coherent extension.

The subsequent combined JIT run passed 621 and failed 113 tests. Six failures
were independent; the remainder followed a poisoned shared Python test lock.
Two tests had selected compiler helper operations instead of the intended
source operation, and the native source-position reader lacked an empty-line-
table preflight. A separate semantic defect tied normalized error forwarding
to suspended activation shutdown, preventing ordinary helper inlining.
`TermRaise::disposition` now distinguishes `Source` from `PropagateNormalized`
independently of handled context. Ordinary cleanup uses Unwind; suspended
cleanup retains Terminal. Generated propagation skips source normalization
callbacks and implicit context chaining in both JIT and deoptimization.
The suspended no-inline guard remains intact. Combined after-validation is
pending; the 436-test result predates this additional separation.

Six additional ordinary generator controls pass. Their twelve genuine strict
counterparts all fail on f1bb/v17 in **71.35 seconds** with unchanged inputs:
`work/strict-v17-generator-delivery-before-f1bb/result.json`. Active delegation
must deliver even missing-throw and delegate-close errors through its existing
StopIteration handler. Temporary delegate and normalized-exception owners must
be released before the resumed source handler completes. These negatives
drive explicit `YieldFromException` delivery and a GC-visible, one-owner
exception handoff; they are not native integration successes.

Three native finalizer controls require closed generator state and no frame
before source-local destruction. All six f1bb strict counterparts fail in
**35.67 seconds**, with unchanged snapshots, in
`work/strict-v17-generator-terminal-before-f1bb/result.json`. A private native
kernel control independently reproduced the ordering bug in all three modes:
the callback had not yet returned, so finalizers still saw a running generator.
The private two-phase terminal notification publishes closed state without
clearing its owner; existing handled/local cleanup then runs, and native final
retirement clears ownership after the callback returns. Its three controls and
all **26 private kernel tests**, **291 native tests**, and **4,118 CPython cases**
pass in `work/cpython-managed-generator-candidate/gate-draft4-full.json`.
Shared-source PGO and the compiled consumer are still pending.

The genuine suspended-transport before family passes five and fails four
outcomes in **71.00 seconds**. Both strict entries retain an original payload
past its last handler use and past supported C-API replacement; the native
controls and except-star group cases pass. Exact unchanged inputs are recorded
in `work/strict-v17-suspended-transport-before/result.json`; the same signed
publication is retained for the next runtime after-replay.

The next source gate passes all **439 lowerer tests**, all original source-
position/namespace/transport failures, and three new raise-disposition tests.
The latter check actual native helper calls, stale typed-plan rejection, and
deoptimized exception identity/context/refcounts. An initial helper-count
assertion included the shared error exit; the corrected test compares the
same-operand Return baseline and requires exactly one added source-raise call
or exactly one normalized restore, not a relaxed count threshold.

The full optimizer gate exposed seven regressions. One test incorrectly
classified all cleanup jumps as loop backedges; it now checks the actual
normal-CFG cycle containing the Next operation and its physical iterator slot.
The StopIteration observation proof treated retirement of its proposed None
input as an arbitrary finalizer. It now proves that exact transport remains
None, while retaining all other callback checks and declining arbitrary
rebinding. All seven original failures pass; an added negative needed the
existing conditional-next fixture because the test inliner does not inline an
always-raising method. Its corrected focused case passes. The first failed
fixture is not counted as a semantic failure.

Generator-consumer budgeting also charged a retained, non-inlineable resume
body as copied code. A shared activation-safety precondition now keeps the
body out of line without charging its size to the caller. Actual consumer and
protocol costs, the 64-block/512-instruction copyable-body bound, and the
384-block/4096-instruction aggregate bound remain. A genuine large-generator
positive and same-sized ordinary-CFG negative pass. Local nqueens retains its
matching planned resume again; a cross-module consumer fixpoint still exceeds
the unchanged block budget and is under investigation. This is not yet a
complete JIT gate or a measured performance result.

The cross-module fixpoint was rediscovering explicitly retired fallback calls
after merging fresh builtin candidates. Re-inlining those cold paths consumed
the budget before the remaining hot consumers. Final admission now filters
every merged candidate family through the existing retirement sidecar. Copying
an already-retired instruction carries that state through qualified function
and instruction identities, including transitive copies; retiring a replaced
source does not retire its newly created hot-path copy. Focused tests cover
both cases and a same-numbered instruction in another function.

The original N-Queens source now produces **274 blocks / 1,486 instruction
nodes**, versus **379 / 2,011** in the failed candidate, with the same budgets.
Its structured regression proves that both set helper bodies were inlined and
that all remaining rediscovered consumers are executable, explicitly retired
cold fallbacks. It no longer incorrectly requires raw discovery to be empty.
The fixed-source gate in `work/strict-v17-final-retirement-after/result.json`
passes **739 JIT**, **439 lowerer**, and **229 optimizer** tests, including the
focused disposition, authentication, transport, retention, and budget tests.
Input snapshots are identical. This establishes the compiler decisions, not a
runtime lifetime result or a throughput improvement.

The coherent v17 after-extension is `00caf03e0af3…` with unchanged native v17,
checker 0020, and Python support; its build took **34.54 seconds**. The full
provenance is `work/strict-v17-disposition-retirement-extension-ready.json`.
The pending-lifetime, suspended-transport, handled-state, contextual-call, and
generator-expression after-replays use this fixed artifact. Their results are
not implied by the compiler gate above.

The first actual after-replays on `00caf03e0af3…` pass all **20** cancellation,
handler-finalizer, and pending-finally cases (**103.25 seconds**), all **8**
generator-expression creation-denial/original-code cases (**87.54 seconds**),
and all **10** contextual-call cases (**95.70 seconds**). Their runtime,
authority, source, and fixture snapshots remain identical. Receipts are
`work/strict-v17-00caf-transport-lifetime-after`,
`work/strict-v17-genexpr-denial-after-00caf/result.json`, and
`work/strict-v17-context-after-00caf03e/result.json`.

The suspended/augmented replay passes **11 and fails 4** in **62.37 seconds**,
with unchanged snapshots. All six augmented-assignment cleanup cases now pass,
as do the three native suspension controls and two strict except-star group
cases. Both strict entries still retain the old exception after handler exit
and after supported handled-exception replacement. The failed evidence is
`work/strict-v17-00caf-suspended-transport-after`; the next repair must account
for every physical copy introduced while lowering preserved block arguments.
The successful non-suspended retirement proof does not close these four
failures, and native promotion remains gated on their correction.

The handled-state after-cohort also passes all **22** cases in **173.85
seconds**, including the permanent profile/apply deoptimization witness, with
identical inputs. Its receipt directory is
`work/strict-v17-00caf-handled-transport-after`. The four bounded ownership
diagnostics then identify two saved exception references at first suspension:
an ordinary block-parameter copy and the separate `_dp_throw_context` snapshot.
The latter is an actual runtime read, not a dead copy that liveness may ignore.
The next candidate removes that snapshot and its slot/constructor fields,
exposing a read-only projection of the existing owned handled item instead.
It separately tags name-binding copies with `StorePurpose` so physical copy
cycles do not manufacture semantic uses. Source aliases and lexical cells keep
their ordinary ownership. Validation of this candidate is pending.

The broader suspended-object controls reproduce another existing incompatibility
on the same frozen v17 artifact: **6 ordinary controls pass and all 12 strict
cases fail** in **106.92 seconds**. They check exact coroutine/async-generator
types, real running state, live-frame inspection or an explicit supported-policy
failure, and rejection of concurrent awaits/async-generator operations. The
receipt `work/strict-v17-suspended-native-identity-before-00caf/result.json`
records unchanged runtime and fixture inputs. The private native kernel is
being extended to these types; this is not selected-runtime after-evidence.

The copy/projection candidate's first gate passed compilation and the new
projection tests, but a new except-star test had selected hidden handler
transports instead of its independent, semantically read saved remainder.
The corrected test follows the post-resume Source-Raise operand and separately
checks a genuinely required pending-return copy across a finally/yield. All
**442 lowerer tests** then pass. The first full JIT invocation also loaded the
new support import against the old extension: a missing export and poisoned
test lock produced **531 passes / 210 failures**, not 210 independent source
regressions. The recipe-backed matching extension must be staged before these
runtime-importing Rust tests; `AGENTS.md` now records that requirement and the
lowerer's Rust 2021 syntax boundary.

The coherent after-extension is `f239f6896794…`, Python support
`8561b37c6327…`, native v17/checker 0020 unchanged, built in **33.86 seconds**.
`work/strict-v17-copy-projection-matched-gate/result.json` has identical source
snapshots and passes **741 JIT**, **442 lowerer**, **229 optimizer**, and all
focused producer/map/consumer/projection checks. The original suspended and
augmented runtime replay then passes **all 15 cases in 64.83 seconds** with
identical publication, fixture, and runtime inputs:
`work/strict-v17-f239-suspended-transport-after`. This closes the four original
handler-retirement/C-API-replacement failures in both entries.

The wider fixed-`f239` replays also pass all **20** cancellation, handler, and
pending-finally cases in **103.67 seconds**, then all **22** handled-state cases
in **151.12 seconds**. The actual verification counters contain exactly **two
permanent deoptimization handoffs** for `deopt_handled_add`. All 57 outcomes
retain byte-identical runtime, publication, and fixture snapshots; the combined
receipt is `work/strict-v17-f239-transport-after-summary.json`. The independent
original-code/creation and fastcall cohorts pass **8 in 87.70 seconds** and
**14 in 66.74 seconds**, with unchanged inputs, in
`work/strict-v17-copy-projection-ty-after-result.json`.

Private native ABI2 coroutine/async-generator controls then exposed a separate
terminal-lifetime issue. An async generator's implicit local finalizer runs
with `ag_running=True` on ordinary return, but with `ag_running=False` after an
unretained exception or close completes. A retained source exception instead
keeps its frame's user locals alive until its traceback is released. Six
ordinary family controls pass; the managed callback has two genuine mismatches
(async exception and close). The initial universal-running assertion was a bad
test assumption, not a native result. Receipts are
`work/cpython-managed-generator-candidate/abi2-terminal-family-before.json` and
`work/cpython-managed-generator-candidate/async-terminal-traceback-control.json`.
Clearing an operation flag or delaying cleanup only until the callback returns
does not preserve retained-exception lifetimes. A source-activation ownership
handoff is required; the native candidate remains unselected. The approved
activation-introspection relaxation does not permit this surviving-user-object
lifetime change. Maintained controls now also cover normal calls and all three
suspended kinds, including local replacement/deletion after a caught exception.
The fixed before-run passes **16 ordinary controls** and **8 strict explicit-
delete cases**, but fails **24 strict escape/retain/replace cases** across both
entries and all four function kinds, in **236.14 seconds**. Every failure is
the actual early-local-finalizer assertion, not checker admission or a setup
failure. Runtime and test-input snapshots are identical in
`work/strict-v17-traceback-lifetime-before-f239/result.json`. The eventual
ownership repair remains separate evidence from the successful transport-copy
work above.

The next source-only checkpoints distinguish producer validation from runtime
evidence. Managed-resume delivery and immutable semantic-None transport pass
**449 lowerer, 11 driver and 229 optimizer tests** after preserving the initial
failed compile/fixture attempts. Parser-owned source-frame inventories then pass
**7 focused projection tests, 25 core, 456 lowerer, 11 driver and 229 optimizer
tests** with unchanged inputs (`work/strict-managed-source-frame-metadata34`).
Four original `_dp_` source-binding controls first fail; all five focused
controls pass after the collector keys original bindings by AST node identity
as well as spelling/range (`work/strict-source-prefixed-bindings-after02`).
The wider suite after that last repair is still pending. An initial joint
JIT/PyO3 test-target check passes in **6.38 seconds**, but subsequent ownership
consumer changes require a new coherent check and actual runtime replay.

Native ordinary controls establish two distinct cleanup orders: **14 controls**
show semantic POP_EXCEPT retirement while source locals remain live, but
residual suspended C-API exception release only after frame handoff/cleanup.
Created-throw controls separately show that the original declaration line is
stable while the native prologue offset varies; the implementation must not
guess a fixed offset. The new lifetime-frame API keeps one lazy frame per
activation, validates the original native local/cell catalogue, and transfers
current owners only at terminal exit. Active-frame inspection/evaluation and
unavailable positions fail explicitly. It does not revoke sealed contracts or
grant execution to an exposed source code object.

The shared-source **v18** native checkpoint is selected and verified at
**2026-08-22 19:12 PDT**, with the host and guest still using the same physical
VirtioFS CPython directory (device 37, inode 24). PGO/LTO compilation takes
**440.25 seconds**. The final optimized-build gate passes **325 native tests**,
**60 development-mode lifetime kernel tests**, exact C/C++ ABI layout checks,
and **4,896 CPython cases across 43 files** (59 skips, 26.7 seconds). All 5,544
source files match the packaged replay. Receipt:
`work/logs/strict-cpython-v18-ready.json`; native generation
`5ed9f08f8223…`, executable `8ba70b38bad7…`, library `c32031bafe43…`.
The prior v17 build remains available. The optional decimal extension remains
unavailable, as before. These native results do not validate the unstaged Rust
consumer, close the 24 retained-traceback failures, or establish performance.

The consumer now represents source ownership explicitly in parser inventory,
resolved storage projection, an immutable native-code selection plan and one
activation-owned handoff. A terminal `SourceFrameExit` operation precedes
implicit suspended-local cleanup. Primary slots, active function/capture pins
and the one capsule-owned snapshot participate in one address-deduplicated
transaction, rather than independent GC shells or duplicate retained snapshots.
The selected synchronous return check moves to the terminal seam after
semantic handler retirement and before frame teardown; an activation marker
prevents repeated checking. Finalizers may reenter strict calls, so borrowed
argument-proof authority retires before provenance references. These source
changes, namespace/hidden-slot coverage and full after-cohorts remain under
integration. **No timed stock/strict comparison or 1.10 claim is available.**

The source-frame-35 pure gate now passes **26 core, 54 typed-IR, 466 lowerer,
11 driver and 231 optimizer tests**, plus the focused marker and actual typed
validator tests. The six-package test-target check takes **8.93 seconds**;
all 196 scoped source inputs are identical before/after
(`work/strict-source-frame35-pure-validator/result.json`). Two earlier failures
were missing propagation of the activation obligation to a public scope. The
generic validator now checks the actual typed function, including a suspended
terminal path with both its marker and producer-boundary tag removed. This is
not validation through a rendered or reconstructed legacy function.

The source-owner scalar before-control passes its ordinary-local positive
control but fails the selected-source-owner policy assertion: the planner still
transports that source local as `ExactI64`
(`work/strict-source-owner-scalar-before/result.json`). The constant in this
small fixture already has an immortal/module-constant fact; this is evidence
of a missing conservative planning rule, **not** a demonstrated finalizer bug.
The candidate preserves boxed representations for locations selected by the
explicit source-frame projection while leaving unrelated compiler temporaries
eligible for scalar transport. Five structured after-tests and the integrated
runtime replay are pending.

The selected native v18 inventory audit passes **112 paired traced/untraced
lifetime controls**, covering 29 sources and 83 original code objects
(`work/strict-v18-hidden-slots-audit/result.json`). Hidden module/class inline
comprehension slots, restored function locals, retained standalone comprehension
locals, synthetic parameters and implicit cells are distinct owner roles.
In particular, an empty native `__class__` cell can later become the actual
method closure cell; manufacturing an unrelated empty cell is unsound.
These controls justify explicit role-qualified producer mappings, not accepting
unknown native slots by spelling or treating every hidden slot as absent. That
producer extension remains pending. The joint source gate preserved an initial
wrong Cargo package name and four subsequent integration compile errors before
retry; none of those attempts executed SOAC or counted as runtime evidence.

The corrected eight-package `--tests` check passes in **3.87 seconds**. All
**five** boxed-source planning tests pass, including paired structured controls
that first prove scalar/truthiness transport without the source obligation and
then retain exactly that compiler-temporary transport with it enabled
(`work/strict-source-frame35-boxed-corrected/result.json`). Two initial positive
fixtures failed because their chosen Python forms did not establish the scalar
shape being asserted; they were replaced with the existing production-shaped
four-block fixture, not by relaxing the source-owner policy. Their failed
3-pass/2-fail attempt remains recorded.

All **seven** native ownership-kernel tests pass, covering current local/cell
capture, address deduplication across owner groups, unique/absent frames,
rejected-transaction atomicity and GC/protocol cleanup. The semantic-handler
and managed-terminal-error tests each pass after correcting their setups:
the former explicitly removes a fixture-created `__context__` reference before
testing an independent pending error; the latter uses the repository's
embedded-Python initialization. The original failures are retained in
`work/strict-source-frame35-handler-retirement` and
`work/strict-source-frame35-managed-terminal`. These checks validate the raw
ownership and planning contracts, not the still-pending actual strict-runtime
traceback and generator-family cohorts.

The first coherent source-frame-35 extension is **`51056611707a…`**, built in
**37.70 seconds**, against selected native v18 and checker 0020/v18. The actual
imported extension, loaded libpython and support files are matched by
`work/strict-v18-source-frame35-extension-ready.json`; its compiler source
fingerprint is `8508abc96c49…`. The checked-return lifetime replay fails both
entries in **20.88 seconds**, with all runtime/test inputs identical, before
either body executes: preparing a source error trampoline switches away from
an unfinished Cranelift block. This is a code-generation failure, not evidence
about return checks or finalizer order. The repair defers those cold blocks
until each source block's terminator has been emitted, without inserting a
jump into the successful path. A structured regression builds the real typed
codegen path and checks SSA validity and cold traceback-helper placement.
Its after-gate and actual replay are pending.

The generator-control-36 producer repair separately changes **three unchanged
compiler regressions from failing to passing** (0.02 seconds of tests). Real
source parameters, locals and cells named like internal controls previously
collided with the preserved layout. Control slots now carry explicit
`GeneratorControlRole`; private resume parameters have a validated role/name
ABI and fresh, source-reserved names. Factory, deopt and optimizer consumers
use those roles. This does not yet remove the independent `_dp_try_*`
heuristics. The combined eight-package test-target check, including new role
invariants, deferred error-block emission and captured environment transport,
passes in **6.94 seconds**
(`work/strict-control36-environment-joint-check02/result.json`). A missing
downstream enum arm was caught in its initial attempt and corrected by rejecting
an unbound resume ABI operand rather than treating it as a null source local.

A native C-API GC discriminator demonstrates that an otherwise-valid terminal
handoff can observe an actual function whose globals and builtins were cleared
by cyclic collection. The live control passes; all **eight cyclic cases** reach
the original finish API with cleared fields and receive `ValueError`, without
direct field mutation. The maintained native before-gate passes **21 tests**
and fails the added test's **eight GC subcases** (0.229 seconds of tests;
`work/cpython-managed-generator-candidate/lifetime-environment-before.json`).
Treating these failures as compiler corruption would be wrong. The next native
API receives the activation's already-owned entry mappings explicitly and
adds no active-frame backedge. Ordinary noop-async-generator-finalizer controls
also show that an unreachable cycle's payload finalizes during collection even
with a retained active traceback; it must not be kept alive by an invented
frame-to-state reference. The actual strict GC replay remains blocked by the
first extension's known codegen bug, including a separately labeled lazy-entry
diagnostic; it is not counted as a GC after-result.

The shared volume had **11,708,592,128 bytes available** before further native
and benchmark artifacts. A real nested-Justfile regression reproduces loss of
the configured pyperformance output root, then passes after preserving
`PYPERFORMANCE_RESULTS_DIR` across child recipes. The before/after receipts are
`work/strict-pyperformance-root-{before,after}/result.json` (1.46/1.12 seconds).
Future task-owned measurements can use VM-local storage without moving shared
sources or prior evidence. This is workflow validation, not a performance
measurement; the fixed 97-driver comparison is still pending.

The final generator-control-36 pure gate passes **795 unique tests**: 26 core,
54 typed IR, 471 lowerer, 11 driver and 233 optimizer. The focused eight tests
also pass, and the six-package test-target check takes **2.16 seconds** with
unchanged scoped inputs (`work/strict-generator-control36-pure-final/result.json`).
Two stale fixture assumptions were corrected: one now tests an illegal async
completion in an ordinary function without first corrupting its resume ABI;
the other expects a user `_dp_state` alias to remain ordinary, while checking
the actual fresh alias and physical-slot cleanup. The root's subsequent
eight-package check passes in **6.87 seconds**, including owned deopt completion
results and preservation of an existing entry error. Terminal handoff rejection
uses a non-unwinding fatal diagnostic instead of dropping an unfinished frame
or committing guessed owners. That fatal policy is **not** a claim that RAII
finalizers run after process termination; recoverable discarded completions
have their own ownership tests. Linked after-tests at this checkpoint still
required the matching native v19 build.

The first captured-environment native repair passed its broad gate but failed
the reverse GC order in **eight of eight actual collection cycles**. An owner
can finish before the function's queued `tp_clear`; merely adding a function
reference does not cancel that clear. The corrected policy checks the actual
function's native COLLECTING bit, plus mismatch of each captured mapping. It
does not use ambient collection state or introduce an active-frame backedge.
All **29** focused lifetime tests now pass, including both eight-case orders
and a reachable-function-during-GC negative control. The final private gate
passes **69 normal + 69 development-mode kernels, 334 native tests and 4,896
CPython cases across 43 files** (47 skips; **43.0 seconds**). Distinct lifetime
ABI2 export, C++ signature, traversal, reentrant clearing, no ordinary map
refcount changes, transactional rejection and no-allocation finish checks pass.
Independent replay matches **all 5,544 source files**; root independently
verified 51 hashed inputs before authorizing the optimized shared-source build
(`work/cpython-lifetime-environment-candidate/private-ready.json`). That private
debug result is separate from the shared-source optimized proof below.

On **2026-08-22 at 20:50 PDT**, the reviewed native v19 build was selected and
the project venv refreshed after a separate root review of its optimized gate.
Patch `0033-source-lifetime-captured-environment.patch` was promoted to the same
physical `/home/adamh.guest/soac/vendor/cpython` source directory: VirtioFS
`lima-63b0316c44311daf`, device **37**, inode **24**, with a bidirectional
host/guest probe. Independent patch replay again matched **all 5,544 source
files**. The optimized build used the unchanged PGO/LTO flags and workload;
neither a private binary nor mixed headers/library were substituted.

The optimized gate passes **334 native tests** (1.182 seconds), **69 normal +
69 development-mode kernels** (0.440/0.571 seconds), and **4,896 CPython cases
across 43 files** (**59 optimized-build skips**, 26.6 seconds). Both eight-cycle
GC orders, the reachable-function negative control, public C++ seven-operand
signature, lifetime ABI2, unchanged physical layouts, traversal and reentrant
clearing checks pass. Source, pin, native-test and binary hashes remain equal
across the gate. These are correctness/build checks, not benchmark timings.

The independently reviewed unselected receipt is
`work/logs/strict-cpython-v19-gated-unselected.json`; the selected/native/venv
receipt is `work/logs/strict-cpython-v19-ready.json`. Exact identities are:

| Artifact | SHA-256 |
| --- | --- |
| Native patch generation | `8bd4a31dc52c6327451f68e5b56059e9edfda56b33b07235dbadce9e054ec0ca` |
| Authored patch 0033 | `0d1e03d2cc0788d8b819b8dc8584a4360511d9a03142f12554bf193c4718ea63` |
| All 5,544 source files | `13a300ec895018afae4dd968e0320175b4b2a8199e334eddc560e3c3a18cd664` |
| Optimized unselected receipt | `3cf7667e634d410a6fd635d9e15a7a5653965db8be917c62bbf9e8be1c90f35c` |
| Selected READY receipt | `55a1968678eb78d1b65330be10eb9dbac64fc69f011872bfa8e683dfdf1b1b11` |
| Optimized interpreter | `85d419f4fbc530d43e25f329fe360ef581b3ad430d1e233fef1a98bb598b096a` |
| Loaded libpython | `71e6fb66c14fa47fce63e90c9f732bd867f8dcd13bb93b9ea68dc00334f59340` |

Actual `.venv/bin/python` reports the base executable under
`/home/adamh.guest/.local/share/soac/builds/strict-opt-v19-01a02587` and loads
that build's exact library **without `LD_LIBRARY_PATH`**. The new
`PyFrame_FinishSoacLifetimeWithEnvironment` export is present and the old export
absent. The persistent v18 build and prior receipts are preserved; its binary
and library hashes were independently checked unchanged after selection. This
native lane staged neither a Rust extension nor a checker. **Actual strict Rust
runtime acceptance, including the GC and suspended-protocol replays, remains
pending a matching extension and its runtime gates.**

Before the next integration/measurement phase, approved removal of only the
obsolete `work/target-ty-source-literals/debug/incremental` cache restored
shared free space to **14.906 GB**; executables, source, logs and results were
preserved. The new task-owned guest-local result root had **91.9 GB free** and
is exposed through the ignored
`work/pyperformance/type-contracts-01a02587-v19` symlink. No existing result was
moved, and this storage preflight produced **no benchmark timings**.

The ordinary no-op async-generator-finalizer control is now maintained in
`tests/test_strict_generator_protocols.py`; its fixed v18 replay passes
**one test in 0.05 seconds** (3.92-second gate). It verifies the finalizer hook
runs before payload destruction despite a retained active traceback, checks
the actual source code identity, and observes the cycle disappear. Its two
strict entries remain queued for the matching repaired extension
(`work/strict-v18-cyclic-async-native-maintained/result.json`).

The next compiler-side repair removes `_dp_try_*` classification from resolved
loads, default constants, exception-carrier lookup and cleanup. A test-first
lookup discriminator fails because a missing source name is redirected to an
unrelated exception binding; the obsolete alias fallback is removed. A typed
storage sidecar carries the original block-parameter roles through the real
optimizer mappings. Ordinary source names receive neither a fallthrough zero
nor an exception-carrier identity. Cache version **37** separates this shape.
Its frozen pure gate passes **801 unique tests**: 26 core, 54 typed IR,
474 lowerer, 11 driver and 236 optimizer
(`work/strict-block-roles37-pure-final/result.json`). The subsequent
workspace-wide test-target check passes in **9.02 seconds** after fixing two
missing imports in the new constant-collector test; that initial failed check
is retained. No production change was needed for that check failure.

Against selected v19, all **eight** linked source-frame kernel tests pass,
including actual function/environment clearing through GC. The two owned
completion tests, two ordinary-source-control-spelling tests and the cold-path
codegen test (seven source forms) pass. Independent review also caught explicit
raise cleanup occurring before the deferred ownership handoff; its continuation
now carries the actual live owners and old exception-edge arguments, and owns
terminal cleanup itself. This avoids a source-side release followed by a cold
path reading the retired value. The genuine unpacked-SetItem selector test
passes for both caught and escaping forms; the four corresponding runtime
cases are still a pending before-discriminator for its separate consuming
operand failure edge. Receipts use `work/strict-v19-source37-*`.

The checker refresh passes all **34 CLI tests** against selected v19 and
preserves the normal binary SHA `05644b448f67…`
(`work/strict-ty-0020-v19-ready.json`). The matching extension build and actual
strict-runtime replays remain pending at this checkpoint. These are correctness
and ownership changes, **not** timed optimization evidence.

The matching extension **`4ead6c9c1fd8…`** subsequently built in **34.17 seconds**
from source fingerprint `be61ac59bacd…`, with native v19/checker 0020 and actual
loaded support hashes verified (`work/strict-v19-source-frame37-extension-ready.json`).
Both real checked-return lifetime entries now pass (**17.78 seconds** of tests,
21.58-second frozen gate), preserving the caller's handled exception and the
source payload until the failed return check's traceback is cleared. The full
JIT library gate reports **757 pass / 9 fail**: seven old deopt-ABI assertions,
one manual block-role fixture, and a tuple-consumer eligibility assertion.
Their after-results are pending; no runtime repair is inferred from a stale
test assertion.

The first four SetItem cases never reach the assignment because eager
compilation rejects an unrelated suspended function's post-handoff writeback.
Splitting only the two new synchronous source definitions into a focused
fixture preserves all six other assignment/suspension cases. All **four**
isolated cases pass (**27.01 seconds**), and the strengthened cases also pass
fresh profile/apply/verify execution (**56.11 seconds**). The structured test
proves a transfer *candidate*, not the additional live `LocalEnv` eligibility;
the suspected consuming-edge bug is therefore **not reproduced** and no repair
has been credited. These receipts are
`work/strict-v19-source37-setitem-{before,isolated-before,profiled-before}/result.json`.
The generator/protocol cohort independently reaches the same source-handoff
validator error; two cross-pass compiler regressions now isolate it after
successful initial lowering and runtime preparation, specifically in resume
preserved-to-local lowering. Its before-gate is **11 pass / 2 fail**
(`work/strict-source-frame-state-before/result.json`).

The resume-state repair updates the current source-owner projection when
preserved values move into active locals; it does not change the original
source schema or weaken the handoff validator. Terminal writeback no longer
recreates those owners after `SourceFrameExit`. Suspension writeback and its
late repair remain. Frozen compiler gates pass **16 driver, 475 lowerer and
236 optimizer tests**, including both previous failures and five new
ownership-shape regressions (`work/strict-source-frame-state-{after-03,full-after}/result.json`).
The workspace test-target check also passes. These changes are not present in
the still-selected `4ead6c9c1fd8…` extension at this checkpoint.

The complete protocol **before** run on that exact extension records
**47 ordinary controls passing and 102 strict cases failing** in 645.09 seconds.
Of the strict failures, **100** stop at the now-isolated resume handoff
validator, while **two** reach the cyclic async-generator body and fail actual
GC cleanup (`work/strict-v19-source37-protocol-full/result.json`). They are
different failures, not one large count of the same compiler error.

The two GC cases are independently traced through native refcount watchpoints
and actual generated-code attribution. The lifetime frame contains only its
original code object while active; clearing its retained traceback does not
release the suspended state. The resumed source body executes **77 INCREFs
and no matching DECREFs** for the generator. Actual compiler probes identify
**135** borrowed-state forwarding preparations: **40** at handled-region entry
and **95** at instruction error edges. Preparation acquires references on the
successful path, but only failure dispatch consumes them. The general repair
therefore defers forwarding and scalar materialization to the cold error edge;
it does not add native frame references or a generator-specific exclusion.
Prepared-prefix cleanup must separately release only newly acquired clones or
boxes if a later box fails, leaving original owners to normal cleanup. The
maintained runtime cases remain red until a matching after-build. Diagnostic
receipts and the two tooling-only GDB failures are indexed by
`work/strict-v19-source37-cyclic-gc-diagnosis.json`.

A separate native-paired namespace test records **three passing controls and
two failing strict class entries** in 21.99 seconds: a retained failed-class
traceback releases its namespace payload too early. The strict module-body
control passes, so the next lifetime projection is class-only, not a blanket
inventory expansion. It must associate the actual original class code solely
for lifetime retention while keeping the synthetic helper code denied as a
public source execution identity. Receipt:
`work/strict-v19-namespace-frame-lifetime-before-4ead/result.json`.

Four more SetItem cases with ordinary and compiler-like source names pass
native-paired none/profile/apply/verify checks in **55.95 seconds**. The
structured source-name transfer exclusion also passes before any selector
repair. No consuming-edge bug has been reproduced; the new structured
`soac.operand_transfer_decision` event distinguishes a candidate from actual
live-local transfer eligibility. The nqueens test's remaining tuple-consumer
call is selected by the real production selector, so its failure is not
dismissed as stale raw metadata; scheduling and legality attribution remain
under investigation. None of these correctness gates supplies benchmark
timings or a full-suite performance claim.

The first joint cold-forwarding compiler checkpoint passes the workspace test
target check in **8.31 seconds** and **768 JIT library tests**. Two tests remain
red: the preexisting nqueens scheduling expectation and the new cold-path
fixture, initially because it attached a source activation after generator
lowering and then because cold refcount expansion creates unmarked child
blocks. The fixture now uses the mechanical compiler path and counts
success-reachable CFG nodes instead of treating layout hints as reachability;
that final fixture revision still awaits its run. The eight obsolete ABI and
manual-layout failures from the previous full JIT checkpoint are resolved.

Extension **`855e4f05ba90…`** builds from fixed source
`3e8f97a80f35…` in **36.33 seconds** with the unchanged selected native v19 and
checker 0020. Actual-import hashes are recorded in
`work/strict-v19-cold-forward37-extension-ready.json`. The native GC control
and both checked-return cases pass, while both cyclic async-generator cases
still fail (**3 pass / 2 fail**, 34.32 seconds). A separate terminal-protocol
after-gate passes **all nine cases**, including the six strict cases formerly
blocked by the resume-state validator. The full 149-case cohort remains
pending, not inferred green from these nine cases.

The same fixed-extension refcount probes show that cold preparation removes
**76 of the 77 retained generator references**. Counts at payload creation,
caught-error callback, completed send, ASend release and post-GC change from
**8/28/81/80/79** to **4/5/5/4/3**. Native watchpoints independently attribute
the remaining **one INCREF and no DECREF** to the resumed source body; outer
creation and native/Rust paths are balanced. The remaining reference is
acquired on the actual caught-error edge and transferred to a target which
treats it as borrowed. A follow-up dispatch sidecar therefore records both
target ownership demand and borrowed-only forwarding, including mixed
owned/borrowed targets, instead of adding generator-name exclusions. These
results are in `work/strict-v19-cold-forward-cyclic-gc-{counts,native-watch}/`;
they are a narrowed failure, not a GC after-pass.

The nqueens diagnostic is now isolated: late optimization sees **380 blocks /
2,584 body instructions**, charges unreachable CFG against the unchanged
384-block limit, then prunes to **325 / 2,515** only after scheduling ends.
Reapplying the selected consumer to the final graph genuinely rewrites one
store; a separate unrepresented activation remains correctly refused.
The candidate fix moves the existing reachability cleanup before late-loop
budget admission and invalidates facts only on actual pruning. Activation
guards and budget limits are unchanged. Tests retain the original expectation
and compare equally sized reachable/unreachable padding. Verification is
pending. The initial diagnostic launch raced Cargo relinking a mutable test
path and produced no result; a byte-verified task-owned executable copy fixed
that workflow error (`work/strict-v19-nqueens-scheduling-isolated-02/result.json`).

The next fixed compiler checkpoint passes **all 776 JIT library tests** in
6.67 seconds (7.17 seconds including the gate), and the workspace test-target
check passes in **5.99 seconds**. The nqueens expectation and its reachable vs.
unreachable padding control both pass. Exception dispatch now carries explicit
target ownership demand, validates borrowed-only transport from actual
parameter/storage facts, and clones only additional owning consumers. The
structured consumer test covers borrowed-only and mixed target orderings.
Two emitted production forwarding loops each pass success, first-box failure,
and second-box failure: **six fault outcomes**. The kernel checks exact owner
release and preserves the original `MemoryError` even when cleanup overwrites
the pending error. Receipts are
`work/strict-v19-dispatch-ownership-{check,jit-lib-fixed}/result.json` and
`work/strict-v19-forward-prefix-kernel-after/result.json`.

The first joint run of those tests recorded **637 pass / 139 fail**. Two test
premises were wrong: the cold-path fixture demanded unnecessary INCREFs even
after borrowed forwarding correctly eliminated them, and the native fault
probe introduced an error from its destructor on the successful path too.
The other **137** failures were verified poisoned-test-mutex cascades, not
137 independent compiler faults. The corrected fixture proves that actual
handled-error paths exist and counts only success-reachable ownership work;
the destructor fault is injected only in the failure cases. Production
ownership checks remain unchanged. Run a new native fault kernel in isolation
before a shared-interpreter full gate, and preserve the first failure apart
from its poisoning fanout. The unsuccessful run remains at
`work/strict-v19-dispatch-ownership-jit-lib/result.json`.

Extension **`ca2b40442b01…`** builds from frozen source
`8b5487616bae…` in **30.66 seconds**, with unchanged native v19, checker 0020,
and Python support. The actual imported library and executable identities are
verified in `work/strict-v19-dispatch-ownership37-extension-ready.json`.
Runtime GC and full protocol after-gates are still pending at this checkpoint;
the 776 compiler tests alone do not establish their behavior.

Six new native-paired class-binding cases expose a separate compatibility
boundary on the earlier fixed `855e4f05ba90…` extension. The corrected
18-case gate reports **12 pass / 6 fail** in **77.33 seconds**: all six native
controls pass, and both strict entries pass plain comprehension targets,
captured targets, and conditional annotations. Both strict entries fail the
`__class__` comprehension target plus method case during lowering, and give
incorrect nonempty restored cells for class-dictionary and lexical-free-cell
shadowing. The native free-shadow control intentionally has an empty method
cell; annotation-provider execution remains interpreted even when its factory
is compiled. These are real binding/capture mismatches, not a reason to admit
synthetic helper code as original source. Receipt:
`work/strict-v19-class-coupling-before-corrected-855e/result.json`.
The narrower next repair resolves the nearest lexical `__class__` owner before
considering an enclosing class. Shared class-cell initialization, capture and
lifetime projection still need one compiler-owned binding recipe; a flat
locals-plus index is insufficient when an inline comprehension temporarily
replaces and restores the cell at that index.

The first actual runtime after-gate on **`ca2b40442b01…`** passes **all five
cases** in **33.81 seconds** (37.39 seconds including provenance capture):
the ordinary cyclic-GC control, both strict cyclic async-generator entries,
and both checked-return lifetime entries. Native, checker, extension, Python
support and maintained test inputs are unchanged across the run. This is the
behavioral after-proof for cold exception preparation plus ownership-demand
dispatch; the previous one-reference leak no longer prevents collection.
Receipt: `work/strict-v19-dispatch-ownership-gc-return-after/result.json`,
log SHA-256 `2f2d57931bf799367c9942e10773157a01382157020151ba6897ab80a601205c`.
The full 149-case protocol replay is running separately; broader compatibility
and performance remain unclaimed.

The complete **149-case** replay on `ca2b40442b01…` finishes with **141 pass /
8 fail** in **827.00 seconds** (830.78 seconds including provenance). All
previously compile-blocked resume cases and both cyclic-GC cases now execute;
the eight remaining failures are four plain-generator protocol cases in each
entry mode. Returning from a caught `GeneratorExit` leaves that error active
during the associated `finally`, where native Python has restored the outer
`KeyError`. A separate fixed-artifact close-return replay reproduces the exact
event mismatch in **18.80 seconds**. Both receipts report unchanged runtime
and test inputs:
`work/strict-v19-dispatch-ownership-protocol-full/result.json` (log SHA-256
`d0a3792859c4edc0b0048eba007e24d40860a70201a4e61f13af5ebeb076e99f`) and
`work/strict-v19-dispatch-close-return-diagnostic/result.json`.

A structured producer regression demonstrates that the caught handler and its
associated finally incorrectly share one exception-region identity. The full
lowerer before-gate is **479 pass / 1 fail**; the repair assigns a distinct
finally exception identity, carrying only escaping errors into that region.
Normal return/break/continue paths therefore leave the caught handler before
the finally executes. All **480 lowerer tests** and **776 JIT library tests**
pass afterward (0.69 and 7.40 seconds of test execution respectively), with
unchanged captured source inputs. The matching runtime after-proof is pending.
Receipts: `work/strict-class-owner-finally-regions-{before,after}/result.json`
and `work/strict-finally-regions-jit-lib/result.json`.

Those lowerer gates also verify the bounded nearest-lexical `__class__` repair.
Its focused before-gate was **9 pass / 3 fail**; all twelve focused cases are
included in the passing full suite. Two intermediate fixture corrections
distinguish ordinary logical cell names, generated storage names, and explicit
raw class-cell aliases. The test still proves that the comprehension callback
and the separate method capture different actual owned cells from different
creators. No runtime class-cell result is inferred from that compiler-only
gate. The complete native class-binding collector and shared
initialization/capture/lifetime recipe remain under development, with the
native patch isolated from the selected v19 sources while runtime gates run.

Extension **`b0080fcf0bb0…`** builds in **30.85 seconds** from frozen source
`37c3b55d2989…` with native v19/checker 0020 unchanged. Its class coupling and
namespace gate is **17 pass / 6 fail** in **98.76 seconds**: the previously
panicking `__class__` case now passes both strict entries. The four restored
class-dictionary/free-cell failures and two class-namespace lifetime failures
remain. Receipt: `work/strict-v19-class-cell-coupling-after-b008/result.json`.

The same build's targeted protocol/GC/return gate is **30 pass / 8 fail** in
**296.74 seconds**, with the identical eight finally-state mismatches. Thus the
distinct-region producer change is not yet a behavioral fix. The subsequent
name-binding argument-completion pass still fills an absent parameter with a
different same-role parameter, undoing the intended region distinction.
The regression is extended through that actual physical-argument pass before
repairing it. Both after-runs retain byte-identical runtime and test inputs;
the protocol receipt is
`work/strict-v19-finally-protocol-targeted-after/result.json`.

A genuine compiler-intrinsic selection probe passes both ordinary controls
and the entry-interpreted case, but fails the compiled optimization assertion
(**3 pass / 1 fail**, **38.48 seconds**). All eight behavior cases across
profile/apply/verify and both entries pass, and baseline probes match real
generator capsules to their native creation owners. However, both measured
apply and verify select **zero** map/filter stages and **zero** generator
instance plans for both functions. Ordinary runtime helpers are not admitted
compiler templates, and the source generator's native activation also remains
protected. This is missing measured selection, not proof that removing the
source-activation boundary would be sound. A separate generic-native-iterator
implementation plan is under review. Receipt:
`work/strict-v19-intrinsic-pipeline-before-b008-retry/result.json`; the initial
isolated-validator import failure is retained separately and is not behavior
evidence.

Inline sidecar reporting now stages candidate mappings and instance-source
rows until the entire multi-target rewrite commits. A later declined target
cannot publish an earlier attempted fragment as completed. The focused
late-target-decline regression passes and the full **237-test optimizer
library** passes (0.62 seconds including the gate). A new
`typed_inline_fragment_committed` event identifies completed target/source
pairs; old attempted binding events and aggregate attempted counts are not
substitutes. Receipts:
`work/strict-inline-transaction-{sidecars-after,opt-lib-after}/result.json`.

The extended finally regression reaches physical jump-argument completion and
fails before the repair: the absent new finally parameter receives the old
caught-handler operand solely because both have the same role. Implicit
transport now requires the exact binding identity; a renamed transfer must
come from an explicit producer edge. All **480 lowerer tests** and **776 JIT
tests** pass afterward, with unchanged source inputs. The initial assertion
used an unsupported `BlockArg` equality operation and is retained as a
compile-only fixture error; the corrected before-test fails on the actual
wrong operand. Receipts:
`work/strict-finally-argument-identity-before-02/result.json`,
`work/strict-finally-argument-identity-after/result.json`, and
`work/strict-finally-argument-identity-jit-after/result.json`.

Matching extension **`88718261c5b3…`**, built in **28.74 seconds** from frozen
source `013eadb9d586…`, passes **all eight previously failing protocol cases**
in **96.13 seconds** (100.17 seconds including provenance). Native v19,
checker 0020, support code, extension and test inputs remain unchanged.
Receipt: `work/strict-v19-finally-identity-targeted-after/result.json`, log
SHA-256 `1d8c1f483ea1c559888d6e56613da5ecaf5ebff84b0c407151f859ba1d5832d0`.
The complete 149-case after-replay is running; this targeted success does not
stand in for the full compatibility gate.

The separate generic-native-iterator strategy is tracked in
[`2026-08-22-native-iterator-pipeline-contracts.md`](2026-08-22-native-iterator-pipeline-contracts.md).
It keeps source generator activations native and does not admit ordinary
runtime helpers as authenticated templates.

The class-binding data model uses dynamic **current native slots**, not a
static allocation generation inferred at a control-flow join. An ordinary
native v19 discriminator confirms that a conditional nested comprehension can
save a LOCAL slot, replace a different FREE slot, and restore only the former;
a subsequent direct read sees the incoming marker on the skipped branch and
the replacement value on the executed branch. Both corrected native-control
methods, covering seven source cases, pass with unchanged native inputs. The
initial later-lambda oracle changed the symbol-table classification and is
retained as invalid evidence. The value-only core/lowerer schema gates pass
**1 + 7 tests**, but the compiler collector is still mirrored and unbuilt, and
runtime class initialization/capture/lifetime integration is not yet claimed.

The complete after-replay on **`88718261c5b3…`** passes **149 / 149 protocol
cases** in **797.74 seconds** (801.07 seconds including provenance). The
selected native executable/library, checker generation, extension, support
code and maintained tests are byte-identical before and after. This closes
the eight finally-state failures as well as the preceding resume/GC defects
for this cohort. It is not a substitute for the remaining class tests or
`just test-all`. Receipt:
`work/strict-v19-finally-identity-protocol-full/result.json`, log SHA-256
`b56976d058a761dcb9cc88446009c93cb60fefdeb778aee68e2a6d3d9131739b`.

The native v20 class-binding package is now applied through the normal pinned
source preparation after exact v19 baseline/archive and shared-mount checks.
The refined eight-file native patch is
`24b815e44966b40b91efbf9d87fb3486977e7ffa0138f0fde9c1b1f79dbf00c0`;
the two-file native-test patch is
`7e32e14d07713c98027814aefefafb518c1d4c86438929e778e2178aa48e408d`.
All 5,544 source files replay identically from the pinned patch generation.
The unselected development build completes in **34.15 seconds** and passes
**21 / 21 focused native metadata/class-API tests**. These include original
coupled sources, conditional FREE replacement, native prefix order, repeated
finally emission, exact source Name/AugAssign locations, and unchanged ordinary
code behavior. Review caught and corrected a source-position drift before
live application; the maintained test retains the native v19 position oracle.
The optimized build and complete native gates remain pending at this checkpoint.
Receipts: `work/logs/strict-cpython-v20-source-promoted.json`,
`work/logs/strict-cpython-v20-development-built-unselected.json`, and
`work/logs/strict-cpython-v20-development-focused.json`.

The Rust class path is being connected to the same recipe for initialization,
captures, namespace exports, and terminal source-frame owners. It has a
validated `ClassBindingSlotOp` with explicit snapshot acquisition lifetime;
saved operands cannot become lifetime-frame rows. JIT and deopt raw transfers
publish all changed slots before release callbacks, and cleanup planning moves
nullable owner state even though the operation deliberately has no ordinary
Store/Rebind action. Native preparation no longer preallocates cells; the
two-argument namespace body must return its actual current class cell.
The data/decoder gates preceding these edits passed **1 core + 8 lowerer + 2
decoder tests**, and `cargo check -p soac_jit --tests` passed at that earlier
decoder checkpoint. The subsequent producer/backend/ABI changes are not yet
compiled together or runtime-validated; none of the six remaining class
failures is claimed fixed by the native-only gate.

The subsequent frozen class compiler/backend checkpoint passes **82 class
compiler tests** (6 core, 71 lowerer, 5 optimizer), **2 nullable slot ownership
tests**, **1 native class-frame projection test**, and **8 native iterator
selection/kernel tests**. `cargo check -p soac_jit --tests` also passes. The
slot tests verify physical-index selection, ownership moves without fabricated
Store/Delete actions, publication before finalizers, and pending-exception
preservation. These checks precede the new consuming-operand and collection
insertion operations and do not close the six actual class-runtime failures.
Receipts: `work/strict-class-slots-iterator-fourth-check/result.json`,
`work/strict-class-slots-core-lowering-opt-checkpoint/result.json`,
`work/strict-native-class-slot-kernels-checkpoint/result.json`,
`work/strict-native-class-frame-plan-after/result.json`, and
`work/strict-native-iterator-structured-after/result.json`.

The final native **v20b** candidate passes **23 focused**, **352 native**, and
**69 kernel tests in each of normal and development mode**, plus **4,896
CPython cases across 43 selected files** (59 skips). This is a selected CPython
regression cohort, not the full CPython suite. Its unchanged-configuration
PGO/LTO build takes **464.43 seconds**, which is build evidence, not a benchmark.
Configured C++/ABI checks, no-LD startup library identity, and independent
5,544-file source replay also pass. The candidate is still unselected at this
checkpoint; matching checker/runtime validation remains required. Receipt:
`work/logs/strict-cpython-v20b-gated-unselected.json`, SHA-256
`bbdf385a38128ccf502ca8c8b46329815f828c0fb025bd87e9ccfe296f9be38c`.
Native generation: `43832252b41a0449f180cdaee7d4113767c3cd7b2e78e095e86ecf82086399c1`;
executable: `460c422e516d03f13c8dcd5ba445c43b78f621d9817b2a83def47d467855688e`;
loaded library: `a78f8acc03fab86647cf38c655957e87d40a37b0e3fdd7490b483e0058c32fee`.

The initial 21-test native candidate rejected valid plain/decorated classes
inside an outer finally. The interrupted first PGO build and two failing
probes are preserved. The correction proves exact original symtable scope
identity through the full parent chain during AddConst canonicalization;
retained-tree parent identity is still checked separately. Both new maintained
regressions run before PGO and pass, as do eight compiler-only standard-library
probes. Source-position and failed-oracle evidence remains in
`work/native-class-bindings-v20/`; no analyzed module initializer was run to
manufacture metadata.

Class comprehensions now require explicit consuming result handoff and exact
native container insertion. `TakeOperand` transfers and clears an allocated
compiler operand owner without an INCREF/DECREF; source-frame, class-current,
saved-snapshot and control owners cannot authorize a take. `ComprehensionInsert`
borrows the container and consumes its owned key/value using native list/set/
dict insertion semantics. Exact Python container type is checked independently
of the storage-role proof. A shared validator rejects a nested take of the
borrowed container. The backend audit found a real exceptional-prefix issue:
dict-key evaluation can fail before a later value operand is taken. Successful
exit state cannot describe that failure edge, so exceptional possible-prefix
owners are being tracked separately. This work and its focused regressions are
not yet compiled or runtime-validated at this checkpoint.

Native v20b is selected at **2026-08-23 00:13:45 PDT** using the normal
selector and venv refresh (0.60 + 6.50 seconds). The selection transaction
rechecks the reviewed receipt, native source/pin/test hashes, loaded library,
and actual venv/base executable without `LD_LIBRARY_PATH`; the original v19
build and evidence are preserved. Receipt:
`work/logs/strict-cpython-v20-ready.json`. This updates native selection only:
checker refresh, matching extension, and actual class/pipeline after-tests
are still pending. The shared volume has under 10 GiB free; the next checker
target is being staged on the guest volume with the original shared cache
preserved, not deleted.

That cache-copy strategy was stopped after a storage failure: the guest's
approximately 90 GiB free was virtual capacity, while its sparse disk shared
the nearly full host volume. Copying the 24 GiB cache exhausted host backing
and caused guest I/O errors. The copy failed before rename/relink, so the
original checker target stayed intact. With approval, the obsolete 11 GiB
checker-test cache and this task's incomplete guest copy were removed; the VM
was restarted and already-free blocks trimmed. At **2026-08-23 00:21:56 PDT**,
all 5,544 native source files, native20 binaries, old19 binaries, selected venv,
and old checker bytes reverified unchanged, with **20,775,772,160 bytes** free
on the shared/host volume. Receipt:
`work/strict-v20-after-storage-recovery.json`. No runtime gate is inferred from
this recovery. The checker now refreshes in its original target, reusing that
same target for CLI tests serially. The refresh driver monitors both artifact
and shared free space; the preserved copy driver now budgets the entire copy
against host backing and stops immediately on reserve exhaustion. README and
AGENTS no longer imply that guest-local storage supplies independent host
capacity. The failed documentation write left the preexisting files intact;
it was reapplied only after recovery.

The normal checker refresh passes its build (**30.12 seconds**) and all **34
CLI tests** (**108.36 seconds**, including rebuilding the test target). The
binary and exporter fingerprints remain byte-identical to v19; this refresh
verifies their unchanged source generation against the newly selected native
epoch instead of inventing a checker change. Both stages reuse the original
physical target serially and finish with over 20 GB shared free space. Receipt:
`work/strict-ty-0020-v20-ready.json`; CLI log SHA-256
`8d939ac51fd53425969f8c38ba4635d8a301d1ebabb138bfb3ee37adeeaad59c`.

The consuming-operand checkpoint passes `cargo check -p soac_jit --tests`
(**7.14 seconds**) and the complete **36 core + 489 lowerer + 242 optimizer
library tests** (**20.14 seconds** including the gate). Source inputs are
unchanged across both commands. Two earlier compile-only fixture mistakes
(missing PyO3/module and BlockPy test imports) are preserved and corrected;
they are not behavior failures. Receipts:
`work/strict-v20-owned-operands-second-check/result.json` and
`work/strict-v20-owned-operands-compiler-libs-second/result.json`; log SHA-256
`4a254cf1dbf1e1c6a7ace2ead16f117103a7d6fb0b4650717b70b663344430fc`
and `fbcc26dd886b542cbf5e5181d5502fa6379f89200e74363f51aabf892c19827f`.
The full JIT library suite, matching extension/runtime after-tests, and final
native-recipe class producer remain pending at this checkpoint.

The subsequent full JIT library run finishes with **790 passing and six failing
tests** in **5.64 seconds** (34.23 seconds including the gate). All six failures
report a missing canonical native class-binding recipe; the class producer is
still unfinished. The consuming-operand entry/kernel, three native collection
insertion kernels, exceptional-prefix cleanup tests, and native-iterator
structured checks pass. Receipt:
`work/strict-v20-owned-operands-jit-lib-second/result.json`; log SHA-256
`6dc6252dece3348b4413076d33de47b4494b5cd069e1114dd992d4d7aa5b1090`.
The earlier run is retained: two newly written fixture errors caused a shared
test-lock poison cascade, and one old structured assertion expected a caller
resume plan even when the selected input template correctly keeps its generator
activation native. The corrected assertion requires two complete native
pipeline plans and zero caller resume plans; it does not accept either shape
indiscriminately. Neither this partial JIT result nor the matching-extension
build in progress closes the actual class-runtime regressions or full gate.

The first matching native20/checker20 extension is now captured as
`d6c3467682c832044831c9c13adf7d1aa78adcdb916c1f85c59022b719a44417`
from unchanged source `9d543970811adb5c028e5a9ec155540b8a55419811c950dc1793c1de58e36246`.
Its actual iterator runtime gate exposes a missing frozen JIT import declaration;
the independent iterator strategy ledger records the 9-pass/4-fail result and
minimal strict repro. No mixed-source build or old extension is counted as an
after-test.

A tests-first lowering discriminator independently confirms an evaluation-order
bug: the conditional argument's predicate executes before its callee and earlier
argument. The pre-branch CFG contains only `predicate`, instead of
`make_callee`, `first`, `predicate`. This is a structured call-order assertion,
not a rendered-IR match. Receipt:
`work/strict-v20-ruff-order-before/result.json`; log SHA-256
`8485123e509831705ea4d72f4a77f77d7c6546a4b5abfcac5fe1e65d452ad25a`.
The bridge now retains compiler-owned operand moves instead of round-tripping
children through raw AST names. Ordered setup and native class-comprehension
production remain under integration, including explicit unpack/merge phases and
ownership across suspension; no broad generator exclusion is introduced.

The bridge/import checkpoint passes **37 core**, **21 expression-lowering**,
and **one immutable-import environment test**, plus `cargo check -p soac_jit
--tests`. The matching `9c8b411dd2c11b3b…` extension then passes all **14 actual
iterator runtime cases**, including required committed optimization bundles in
apply and verify. Receipts: `work/strict-v20-ruff-payload-core/result.json`,
`work/strict-v20-ruff-bridge-lowering/result.json`,
`work/strict-v20-reserved-native-imports/result.json`, and
`work/strict-v20-native-iterator-imports-after/result.json`. These checks do not
close the remaining native class producer, suspended-operand, loop-lifetime,
full-suite, or performance work. The separate iterator strategy records exact
artifact identities, failures, correction, and runtime-after evidence.

An actual native-oracle loop discriminator on the same extension finishes
**one ordinary pass and two strict failures**. Relative reference counts at
the iterator's callbacks match native in compiled mode, but one owner remains
after loop exit; forced-entry mode also has one extra reference during
`__next__`. Receipt: `work/strict-v20-loop-next-before-9c8b/result.json`.
The repair uses explicit borrowing iteration and loop-exit ownership, including
nested handled exceptions, rather than changing the user-visible builtin
`next` or treating generic last use as a loop-scope lifetime. Runtime-after is
pending.

The shared operand representation now supports both local owners and explicit
preserved generator owners. Generator construction initializes operand slots
to NULL, not a Python `None` reference; terminal cleanup clears them in reverse
acquisition order before source-frame handoff and source-local release. The
new publishing API clears or replaces the actual physical owner before a
finalizer can reenter. All **nine preserved-state kernels pass**, including
physical-index/role validation, exceptional cleanup, and reentrant clearing.
`cargo check -p soac_jit --tests` passes on the same source generation. Receipts:
`work/strict-v20-shared-operands39-fourth-check/result.json` (7.12 seconds) and
`work/strict-v20-preserved-operands39-kernels-first/result.json` (9 tests in
0.02 seconds; 55.45 seconds including build). These are compiler/kernel checks,
not an actual suspended-function after-test.

A retained structured nested-yield test fails before the repair: the pre-yield
operation sequence is empty, but must evaluate `consume` and `make` first.
Receipt: `work/strict-v20-nested-yield-order-before39-second/result.json`;
log SHA-256 `9315446c099c893e377ab0400f67cf794e21b0c882843329b549909f3e499744`.
The ordinary controls for call expansion (five cases), loop exits (ten tests),
and suspended expression ownership (six cases) pass on selected native20b.
The matching extension replay is still pending; the older extension cannot
be used after changing the generator factory's preserved-operand ABI.

The first complete JIT library run at this checkpoint reports **560 passing
and 247 failing tests** (807 total, 1.51 seconds). Failure classification finds
**16 primary loop-lowering panics and 231 shared-lock poison cascades**, not
247 independent semantic defects. The producer passed `IteratorStep` through
a synthetic source-AST loop/try round-trip, which cannot represent that
compiler operation. Receipt:
`work/strict-v20-shared-operands39-jit-lib-first/result.json`; log SHA-256
`deb6fa541619409728efc5e15ed30f4bbbc02e668d389fa24bf99f7e13268e91`.
The direct-CFG repair is being checked on focused loop tests before rebuilding
the extension. Its first compiled replay has one pass and three failures at
remaining IR-to-AST statement/return boundaries; the exact backtraces are in
`work/strict-v20-loop-direct-cfg39-backtrace/result.json`. No poisoned-lock
recovery, ordinary-call substitution, or skipped loop behavior is accepted as
a repair.

Four native-only traceback controls distinguish C and Python iterator
exhaustion. Retained `StopIteration` from a C `tp_iternext` has no traceback and
does not retain loop locals; a Python `__next__` traceback may retain its caller
through `f_back` despite not listing the loop frame. Both real-error variants
retain the loop frame. The initial Python-only control incorrectly expected
early collection and is retained as failed-oracle evidence. Receipt:
`work/strict-v20-loop-native-exhaustion-traceback-02/result.json` (4 passes,
0.16 seconds). This is native oracle evidence; strict error-routing afterproof
remains required.

The lowerer fixture adapter now exports the actual privately compiled native
class tree without executing source. Its native-only Unicode/inline-comprehension
control passes (`work/strict-v20-native-lowering-metadata-fixture-control-03/`).
It consumes the existing `CPYTHON_BIN` selection: the shared vendored directory
is source-only after the approved out-of-tree build migration. A first attempt
incorrectly addressed `vendor/cpython/python`, and a second placed its
nonexecution sentinel before the class so native dead-code removal correctly
removed the class recipe. Both unsuccessful controls are retained. The Rust
adapter, actual producer, and class-runtime after-tests remain under integration.
No performance timings or full-gate success are inferred from these checks.

The focused loop gate is now **six passing tests** (0.02 seconds; 18.13 seconds
including the gate). Besides eliminating the source-AST round-trips, it found a
real metadata-loss bug: the structured-to-basic-block conversion replaced the
whole block context with its default and discarded the synthetic transport's
`Unwind` policy. Preserving the existing context prevents loop cleanup from
entering a new handled-exception scope. The new conversion regression checks
all four context policies and suspension/source-exit metadata. The existing
loop assertion was retained rather than weakened to accept the lost marker.
Receipt: `work/strict-v20-loop-direct-cfg39-eighth/result.json`; log SHA-256
`ccbdad920e866af9180d2a955cf6cdbe032c26fdbbca84c60c5e00849ee73dac`.

The matching **shared39** extension builds from unchanged source in **42.84
seconds**, with actual import and native/checker identity verified. Extension:
`d74fcc902a28a7bda8213a108d3ef9f00a709bfa3028f88eaf8a40148785870e`;
source fingerprint:
`e6ca835e7ab030ead9890f67e999312747ce58d74bf7d4e2bf603235992e7aec`.
Receipt: `work/strict-v20-shared39-extension-ready.json`. Later source edits
were deliberately kept separate from this fixed runtime replay.

That actual replay finishes **10 passing / 20 failing tests**, **195.98 seconds**
of pytest time and **199.70 seconds** including provenance capture. All native,
checker, extension, Python support, and test inputs remain byte-identical.
Receipt: `work/strict-v20-shared39-runtime-first/result.json`; log SHA-256
`60eddb4340eab2c3ea051cc92e384a83970b6bb5c6e9602afe1121b4e26b17d8`.

- Both loop-receiver modes and both nine-case loop-exit comparisons pass
  (four pytest tests), including finalizer handled context and return-value
  evaluation before iterator retirement.
- Real iterator errors pass all four C/Python × compiled/entry comparisons,
  retaining the original source code and correct relative traceback line.
- All four normal-exhaustion comparisons fail because an extra source
  traceback is attached. C exhaustion consequently retains source locals;
  Python callback retention must remain intact when that extra frame is removed.
- Both expanded-call comparisons fail. Compiled singleton expansion reports
  an unbound internal operand; entry mode exposes independent conditional-result
  lifetime and pre-yield order problems. The original five controls and the two
  new suspended controls remain in the same maintained fixture.
- All twelve suspended-expression cases fail before the ownership comparison:
  the first `next()` has not evaluated the earlier operand at all. This agrees
  with the retained structured ordering discriminator, rather than proving a
  defect in the already-passing preserved-state kernels.
- Attribute assignment passes both compiled cases but fails both entry cases:
  the callback sees reference count **3**, versus the native **2**. Finalizer
  order/context otherwise matches. The existing compiled borrow plan applies;
  the entry evaluator clones the compiler operand. The paired native-only
  success/error controls pass without importing SOAC.

The source repair for nested-yield order shares the call-phase builder with
conditional argument lowering. The combined JIT test-target typecheck passes
in **9.13 seconds** (`work/strict-v20-yield-class-owner39-second-check/`);
the **13 nested-yield** and **five Ruff call-phase** structured tests pass.
The original pre-yield ordering failure is now green. The **13 trusted-owner**
tests also pass, including borrowed loop stepping and consuming iterator
transport; this is analysis evidence, not a claim that the inliner selects
the new operation. The class fixtures now receive
actual native recipes through the compile-only adapter and production
`CanonicalClassBindings` validator; its native class-tree Rust fixture passes
without executing the source. New class producer, exhaustion policy,
conditional-result, and entry assignment changes must each obtain fresh
fixed-artifact afterproof; the full gate and benchmark measurements remain open.

Cache format **40** separates class-comprehension emission identities from
native owner identities. Repeated source lowering, including duplicated
finally paths, receives separate physical snapshot owners rather than
weakening acquisition-rank validation. Source-error dispatch now carries an
explicit policy: only canonical implicit iterator advance omits adding the
loop frame for pending `StopIteration`. It leaves the pending exception,
callback traceback, and source-frame finish unchanged. These source changes
are not in the retained `d74f…` extension and await the next frozen build.

The expanded native traceback control now passes **six cases**, including C
and Python explicit `next()` exhaustion. The latter keeps its source traceback
and payload in both unchanged strict runtime modes (**four cases**, 27.30
seconds); it is the negative control for the new implicit-only policy, not an
after-result for that policy.

The generic local-package preparation repair is integrated. Its **55 tooling
tests pass** in 1.71 seconds, covering accepted-payload cache identity, equal
portable payloads across environments, coherent payload/RECORD mutations,
idempotent environment selection and rejection of incompatible comparisons.
Fresh real dependency preparation and offline publication of **2to3 pass** in
13.51 seconds using native v20b and checker v20. The original driver and local
package sources are unchanged; the suite's own vendored `lib2to3` is installed
before analysis in the selected benchmark environment. This removes the known
dependency-preparation blocker without proving worker completion, strict hot-path
coverage or performance. Retained evidence is under
`work/pyperformance/strict-v20-lib2to3-prepared-first/`.

The next fixed **97-driver** offline preflight completes in **1,159.30 seconds**:
all **97 dependencies prepare**, **54 drivers publish contracts**, and **43
are rejected**. Inputs remain unchanged. This adds the repaired `2to3` driver
to the previous 53 publishing drivers; it is not a passing-subset benchmark
selection. Receipt:
`work/pyperformance/strict-v20-suite-offline-prepared/result.json`. Rejections
include genuine return-annotation contradictions, unresolved cooperative mixin
methods, and exporter consistency errors. They remain visible failures, not
ordinary-SOAC substitutes or suppressed diagnostics. Worker completion,
transformed hot paths, full-suite comparisons, and performance timings remain
unmeasured at this checkpoint.

The first combined cache-40 source gate catches a missing explicit owner-type
re-export and two remaining callers of the changed failure-site interface;
the corrected `cargo check -p soac_jit --tests` passes in **10.17 seconds**.
Focused tests then pass for six class-metadata cases, two class-slot cases,
two conditional operand cases, explicit exception-edge preservation, three
late iterator selections, iterator alias transport, and both raw source-frame
policy/cold-code paths. These are structured/compiler checks, not a rebuilt
runtime result. Receipts use the `work/strict-v20-*-40-*` labels recorded beside
the preserved logs.

The singleton expansion's structured discriminator is genuinely red before
the repair: an immortal module constant forwarded through a block parameter
was assigned a stack mirror despite its producer emitting no stack write.
The existing local-environment decision now keeps an immortal SSA value local;
the same test passes after the change. Receipts:
`work/strict-v20-singleton-owner40-before/result.json` and
`work/strict-v20-singleton-owner40-after/result.json`. No source-frame/deopt
consumer workaround or rendered-code assertion is involved.

Native captured/chained assignment controls pass **seven tests**. The unchanged
`d74f…` runtime then fails both strict modes (**two tests**, 16.05 seconds;
19.48 seconds including provenance). Compiled execution omits the native-owned
copy for an earlier chained target; entry execution adds extra receiver/value
references. Evaluation, no-GC retirement order, and handled context otherwise
match. The retained log is
`work/logs/strict-v20-setattr-captured-shared39-before.log`, SHA-256
`14848d5c7ec47c905a37e438c037ce43a517565d0a3ff71261a56e90285f10b1`.
The planned repair makes the native copy/move handoff explicit in resolved IR;
source-local borrowed assignment remains a separate required control.

The retained v20 offline rejection audit verifies identical Python source bytes
and checker stderr for all **43** remaining failures against the prior audited
generation: **16 cooperative-mixin**, **nine `sys.modules` sentinel model**,
**five nullable/dynamic-member**, **four callable-shape**, **four declared-return**,
**two legacy-error-path**, **two exporter-consistency**, and **one override**
case. No previously publishing driver regressed. Receipt:
`work/strict-v20-benchmark-rejection-audit.json`, SHA-256
`999c3008972cc45028705bcdb237c25ad9f0800962fa7df3a32007fd25a77cd8`.
This retains the classification evidence without rerunning source initializers
or constructing authority from diagnostics.

The source-local assignment discriminator is independently **red in both
strict modes**: callback reference counts are **3 compiled / 4 entry / 2
native** for both success and error; release order and handled context still
match. Receipt:
`work/strict-v20-setattr-source-local-shared39-before/result.json`, log SHA-256
`faae7ffdb41839a1916de6512704148314d59d342a809af04331f7fdd0c57bb3`.
Pinned native Python calls transfer stack references into the callee, whereas
the custom-vectorcall route retains its caller operands while the Rust binder
acquires bound arguments. Entry `Load` adds another temporary reference. A
correct repair requires an explicit ownership handoff, including incoming
borrowed tags and callee deletion/rebinding; removing the public borrowed-ABI
`INCREF` is not safe. This remains separate from the tested factory/captured
assignment copy/move repair.

Three native-only class-prefix controls pass in **0.02 seconds**, including
failure before comprehension entry and failure during iteration. Cell cleanup
on the latter path precedes retirement of an older call argument, without a
forced collection. The unchanged `d74f…` strict replay fails both modes before
this behavior because that build has no canonical class recipe producer.
Receipts: `work/strict-v20-class-prefix-native-first/result.json` and
`work/strict-v20-class-prefix-shared39-before/result.json`; the latter log is
`fc1a0d855019dad554613082978f2d564e9aa03bf1102f854705af9920693a0c`.
This is admission-before evidence, not a measured prefix-cleanup defect.

The corrected canonical class producer passes **19 structured tests**. The
class-prefix planner test now proves that the actual prefix binding stays
owned through cell restoration, is present in the deopt-resume ownership plan,
and is released only by the subsequent explicit deletion. Its borrowed SSA
view is not the physical owner; the first test assertion confused those roles
and was corrected without a production ownership change. The sealed
class/method kernels pass **ten tests**, and the code-catalog tests pass **two**.
The retained generic source-error-site tests pass **three tests** after the
unreachable Python-method iterator specialization was withdrawn; actual native
iterator/materializer plans remain separate. These checks do not replace the
pending fixed-artifact runtime replay.

The first full JIT-library run reports **685 passed / 129 failed** out of 814.
There are six initial failures followed by 123 poisoned-lock cascades, not 129
independent defects. One initial failure exposes a real transfer-order bug:
consuming a setter operand erases its function fact before callable-field
analysis records the store. The remaining failures require auditing the
explicit iterator operation and current CFG against old fixture selectors.
The failed receipt is `work/strict-v20-class-handoff40-jit-lib/result.json`,
log SHA-256
`01cef0a46edb561b14b5f77250dc8c21a54c746ee3dda6722ff0b202ef99baad`.

The native argument-handoff controls now pass **22 cases** in **0.06 seconds**,
without loading SOAC. They distinguish owned factory inputs, caller-local
borrowed inputs, aliases, early deletion/rebinding, the public borrowed C
vectorcall API, and cold/warmed expanded calls. On unchanged `d74f…`, the two
strict aggregates fail in **16.12 seconds**: compiled execution disagrees in
**21/22** cases and entry interpretation in **20/22**. Expanded-call keyword
containers retire after the strict body instead of before it; the compiled
public-C error case also loses a source-frame owner before traceback clearing.
These are actual runtime failures, not inferred ABI requirements. Receipts:
`work/strict-v20-source-arguments-native-first/result.json` and
`work/strict-v20-source-arguments-shared39-before/result.json`; the latter's
`audit.json` lists the first differing event per case. Log SHA-256:
`fbf63065827680d9526b622844609baf8fabcd2afd38320673c55f17641caa6e`.

The next frozen extension is **`748747c7…`**, built in **41.03 seconds** against
the same native v20b/checker v20 pair. Its source fingerprint is
`84be53686b05eba8b27adab5adb5634b003005faa7c1d16ca6012c9707202016`.
The real callable-field regression is fixed by capturing the stored function
and receiver identity before consuming operands; its focused transfer test
passes. The full JIT library improves to **813/814 passing**. The remaining
loop-split failure exposes another semantic-identity bug: a hoisted `None`
constant is tested by its former name spelling. The proposed fix uses the
existing module-constant classifier and retains a false-global negative case;
it has not yet passed its post-fix gate.

Fixed-artifact runtime replays give the following results. These independently
isolated correctness cohorts ran concurrently; their wall times are not
performance evidence.

| Cohort | Result | Retained evidence |
| --- | --- | --- |
| Original 30 lifetime/call cases | **24 passed / 6 failed**, previously 10/30 passed | `work/strict-v20-shared40-runtime-first/` |
| Expanded 40-case cohort, including new argument-boundary controls | **30 passed / 10 failed**, 282.42 s | Same receipt; log SHA-256 `6ea0e00b78be150cb91ceb53a6a5556ac0af2e61a3661a514a9185ba3f3b251a` |
| Class cells, prefix cleanup and failed namespaces | **9 passed / 8 failed**, 116.43 s | `work/strict-v20-shared40-class-runtime-first/`; log SHA-256 `1c313462d6f993ffa1ca0bce59d4760310e81d76927247e3fbcf543c5adcc566` |
| Existing native iterator/materializer optimizations and controls | **14 passed**, 152.12 s | `work/strict-v20-shared40-native-pipeline-first/`; log SHA-256 `8db3e5d4c6a0625d166dfd6ff6c1a20dbfe0260c5f0927858fc4e50b7ac8f42c` |

The expanded-call and factory/captured-assignment fixes now pass in both entry
modes. C-slot exhaustion omits its source traceback and releases the payload;
all explicit-`next` controls still pass. Python callback exhaustion now omits
the extra traceback correctly, but still loses the source-local lifetime that
native callback-frame ancestry retains. This requires a real active source
parent, not reattaching the forbidden loop traceback. Suspended expressions
now evaluate before yielding and clean up correctly in eight of twelve cases.
The two resume failures are outbound argument ownership (callback reference count 2
instead of 1); the two injected-throw failures lack their original-yield source
traceback and retire the source local early. The four new incoming-argument
aggregate failures remain part of the separate native-reference handoff work.

The class failures are narrowed to three concrete boundaries. The final
construction pass overwrites canonical cell-export flags using an obsolete
generated-name scan. Native class-variable annotation providers use an
authenticated body-completion marker, not an original source span. Finally,
the non-exhaustion iterator branch reaches class-cell restoration through a
normal CFG edge and bypasses partial-result cleanup. Repairs are in progress
with structured assertions on the actual final constructor flags, capture
provenance, and exception-edge retirement. No full-gate or benchmark-completion
claim follows from these selected runtime results.

#### Source-site and class-capture checkpoint 41 (August 23, 2026 PDT)

The canonical class repairs now pass the actual runtime boundary, not only
lowering tests. Native cell-export flags remain authoritative through final
construction; annotation-provider body completion has a distinct validated
capture origin; exceptional comprehension cleanup releases the partial result
and iterator before restoring class cells, and the saved prefix afterward.
The rebuilt extension is
`9481cda38965cee3bbfbe9509ee7927e4f1cd120ab30607ce1631a41200b0eaf`,
source fingerprint
`d51f0b3b99763c467da5f690761c92e4b7ae81cb9216d1b8182ed3698ac3ab18`,
built in **40.23 seconds** against unchanged native v20b/checker v20.

| Maintained runtime cohort | Result | Evidence |
| --- | --- | --- |
| Class cells, prefix cleanup and failed namespaces | **17 passed**, 116.49 s; previously 9/17 | `work/strict-v20-shared41-class-runtime-first/`; log SHA-256 `5240bbdb9aa543a7c7832b6c00d8cedebb3555fa0854715130a3ab644f85c9a6` |
| Suspended expression operands | **10 passed / 2 failed**; direct throw now passes in both backends | `work/strict-v20-shared41-generator-runtime-first/` |
| New delegated-throw controls | **2 passed / 4 failed**; all three ordinary-native controls pass | Same strict receipt; native control `work/strict-v20-yieldfrom-source-site-native-first/` |

The combined generator replay took **107.66 seconds**, log SHA-256
`9906655cafcc3205c2d12e7a07ff1b0c3ae68da1e73001e6ef0fa8f31442d4be`.
These are correctness timings, not performance measurements. Both receipts
verify unchanged actual native/checker/extension/support and test inputs.
Direct resume injection is now a fallible statement carrying the exact original
yield source range; its normalized propagation remains a non-source event.
Two structured injection tests pass, including preservation after exception
splitting. Delegated exceptions expose the next distinct gap: an escaping
error needs the outer yield-from source frame, but a delegate's consumed
StopIteration completion must not gain one. The new tests prove both sides.
The remaining two ordinary resume failures still observe an extra outbound
argument reference; this source-site repair makes no native-reference ABI claim.

The semantic `None` classifier passes its three focused split tests and the
actual profile/apply counter test without increasing a budget. The complete
JIT library now has **813 passed / 2 failed** of **815** in **19.38 seconds**
(`work/strict-v20-checkpoint41-after2-jit-lib/`, log SHA-256
`81d9d8de3ea15eb7df276f81ec95d46bb39a4c80b12cf81475244feb35401483`).
The two remaining failures concern generator resume-plan coverage and residual
consumer retirement after alias continuation cloning; their original structured
predicates remain intact while the production decision/budget path is audited.
The full gate and fixed-suite benchmark evidence are still pending.

#### Explicit normalized source events, checkpoint 42 (August 23, 2026 PDT)

`RaiseDisposition::SourceNormalized` preserves the already-normalized exception
and carries one exact original source event. Direct resume injection and only
the escaping branch of delegated throw use it; a consumed `StopIteration` and
ordinary normalized forwarding do not invent an outer event. Shared validation
checks source containment, explicit exception operands, inline caller-site
overrides and the prohibition on source events after `SourceFrameExit`.
BlockPy cache version 42 preserves the event range through archive/remapping.

The first full JIT run exposed a producer defect rather than a reason to weaken
validation: synthetic generator-expression Yield nodes retained template
offsets. The producer now copies the original element range before rewriting,
matching native `COMP_GENEXP`'s `ADDOP_YIELD(c, elt_loc)`. A structured regression
follows ordinary and conditional multiline element ranges from the original
AST through final normalized source events. The 19 affected primary failures
and 120 lock-poison cascades disappear. The subsequent JIT library run has
**815 passed / 2 failed**, of 817, in **20.57 seconds**; the remaining original
diagonal/nqueens planning assertions remain intact. Source/check/archive,
four managed-generator lowering tests, two raise-disposition validations,
the post-exit negative check and driver cache/remapping test pass.

The rebuilt extension SHA-256 is
`02db2612884945f828b9d22f90c1199ef1bfc0adba2d7826689e6af111ef9d12`,
source fingerprint
`cc480795eb9dba6611f46612120fd7f54ba55168f13caf48142132075c751c42`,
built in **41.36 seconds** against unchanged native20b/checker20. All **eight
direct/delegated throw controls pass** in both compiled and entry modes,
**55.07 seconds**, including the four previously failing delegated cases.
Receipt `work/strict-v20-shared42-throw-runtime-first/result.json` verifies
unchanged runtime/support and test inputs; log SHA-256
`aca4bb67831a2921abbb3d80073425820285b79749f90dbe3f5c821a1e4795d9`.
These are correctness timings, not benchmark evidence.

The actual `just test-all` attempt stopped after **203.61 seconds**. Its Rust
workspace-link phase hit a kernel-confirmed OOM from concurrent linkers in the
12 GiB VM; the standalone raw runtime phase passed all **eight tests**. Pytest
collected **2,807 nodes** into 136 batches, but the disk guard interrupted it
when shared free space dropped below 8 GiB. Thus there is **no completed pytest
or full-gate result**. The full-gate log SHA-256 is
`39abea137794dc4e96975dd130684d04bcea4d784e748b3249bcd58c59d45c06`;
the kernel log is retained at
`work/logs/strict-v20-fullgate42-first-kernel-memory.log`.

The interrupt exposed orphaned worker process groups. All eight recorded groups
were explicitly retired and verified absent. Four real process-level regression
controls reproduce cancellation defects before the runner repair (two failures,
two errors). The runner now cancels queued work, serializes process publication
against cleanup, and terminates whole groups using one shared grace period even
after a leader exits. It preserves diagnostics and the original signal/error.
The first repaired run exposed a fixture-only reentrant `subprocess.wait()`
deadlock in its signal handler. Native stacks show the worker blocked acquiring
a lock during signal dispatch, with its child already a zombie; the capture's
nonzero status reflects the attempted attachment to that exited child. Moving
fixture reaping outside the signal handler fixes the test without changing
production or test deadlines. The final five workflow tests pass in **6.57
seconds** (7.15 seconds including the gate), with scoped Ruff clean. Log SHA-256
`8b3cf524806c6e3a40d1eaa00edceb05c29729810cb8f0d0b63946051589b568`.

The full-gate recipe now serializes compiler jobs as well as the test harness.
Explicitly approved removal of only generated Rust incremental state restored
about 29 GiB free; all selected native/checker/extension and pin hashes remain
unchanged. A repeat full gate is pending. The separate native21 source-parent
scope and native22 token primitives remain ignored, unbuilt drafts, not
validated runtime implementations. Fixed 97-driver strict benchmark timing,
stock comparison and previous-strict comparison remain pending.

#### Late planning and operation-owned reads, checkpoint 43 (August 23, 2026 PDT)

The late inliner now drains eligible executable calls before optional
continuation cloning. An idle split that changes the graph invalidates cached
owner facts and resumes selection; successful-inline paths no longer clone
eagerly. The existing 384-block / 4,096-instruction limits and activation
admission rules are unchanged. This restores the diagonal structured regression.

The N-queens fixture then exposed two different issues. Its old requirement for
at least one redundant cold consumer was incidental, so a stronger positive
activation check replaced it. That new check found a real production defect:
dead-materialization liveness visited only `Load`, missed `TakeOperand` and
`IteratorStep`, and removed the named generator call and iterator acquisition
while retaining a raw step of the preserved owner. Original/final typed call
inventories proved actual removal, not merely lost sidecar metadata.

Both liveness and diagnostic-context collection now use the same explicit
operation-owned binding reads, including comprehension, class-slot and call
argument operations. Copy-chain exclusion identifies one exact borrowed IR
`Load` node instead of suppressing all reads of its physical location. The new
small source-derived local/preserved regression fails before the fix (expected
`Local(2)`, found no read) and passes afterward. Its before log SHA-256 is
`134ccd3b20ee31cc6937613e509298ce6fff392cb9b99b3e59b2139483b4ad5d`;
the passing focused log is
`e2efe5bfb5c6b76a3fbe4467dbafff6972fc3313dfcdce80e5c2cad1b2fe8395`.

The N-queens check retains every original call ID, generator target and argument
plan, factory, and suspended layout. It separately checks the two planned inner
resumes and the original raw native iterator boundary; a native `IteratorStep`
does not require a Python-method resume-inlining plan. Diagonal and N-queens
now pass. The entire JIT library passes **818 / 818** in **16.81 seconds**
(17.20 seconds including the gate), log SHA-256
`1087f0fd066b24294ad572a010ec059c4e6d3f52cc5dbeaed137f0f213ed8748`.
The actual before-build native/compiled/entry runtime controls also pass all
**three tests**, **38.44 seconds**, covering none/profile/apply, acquisition
order and callbacks. This smaller runtime workload did not reproduce the
optimization failure; it is compatibility coverage, not runtime RED-to-GREEN.
Its log SHA-256 is
`05ecf943a8b97284c753a0277cd945cc4ecf1e49ad907c9558de23adf1cb055d`.

One attempted Cargo check omitted the Justfile-selected native environment and
failed before Rust checking because it searched the shared source directory
for libpython. Re-running through `just --command` passed. The focused driver
now rejects that launch early, and README/AGENTS document the selected-build
environment. This was not a different CPython source checkout.

The matching runtime rebuild passes in **37.44 seconds**, extension SHA-256
`fbf57cf57611b9b17af521de0e21f3d935034c5baaee270f1ea753bccba81af0`,
source fingerprint
`37aa6dc5085a6b724c1fcb72bd59e1167f447e6ceb7688c3bc5a64a1082e449c`.
Actual import provenance matches unchanged native20b/checker20. The full-gate
retry completed in **1,529.25 seconds**, exit **101**, with unchanged compiler,
native and checker inputs and no disk-reserve interruption. The JIT library
again passed **818 / 818**; the lowerer passed **517** and failed **15**; the
standalone raw runtime passed **8 / 8**. Pytest collected **2,814** nodes in
**136** batches: **51** batches passed, **66** failed, and **19** timed out.
The retained log reports 175 distinct failed/error node IDs, but the timed-out
batches contain unexecuted cases, so this is not a complete per-case total.
The full-gate log SHA-256 is
`8e92dce7409c66b22f8a95a1655a31936bb3a790951cd8dfe43b5557e24018eb`;
the terminal provenance receipt SHA-256 is
`ca711422143c95b1f64f5ea4d46f936234994f8b0429f314d54ba6186efbfef3`.
This is a failed baseline, not runtime acceptance.

The lowerer failures include generated async suspension ranges, a nested
closure using the callee's incoming carrier instead of the creating frame's
cell, annotation capture rewriting that overwrites dictionary-first lookup,
and stale assertions on previous representation shapes. Repairs preserve the
source scenarios and distinguish producer bugs from obsolete assertions.
The generator-worker timeout capture found active JIT planning; it does not
prove that every timeout has the same cause. The runner sizes batches from the
whole suite, and integration fixtures execute entire reviewed cohorts in both
modes even for a small selection. A bounded batch ceiling therefore needs
explicit selected-case/mode propagation as well, not merely smaller batches.
The capture also exposed a missing Python stack helper lookup for out-of-tree
CPython builds; native stacks were available, but Python `py-bt` was not.

A separate audit is checking whether unused named-generator call removal has sufficient
argument-check and lifetime proof; the missing-read fix does not establish that
broader admission condition. No native21/native22 or performance acceptance
follows from these compiler results.

#### Source ownership and bounded compatibility workers, checkpoint 44 (August 23, 2026 PDT)

New failing tests reproduced three distinct producer defects against checkpoint
43: async generator/with implicit suspension ranges escaped their original
expression; a method forwarding a captured cell into a nested lambda/function
used the callee's carrier rather than the creating activation; and unused
generator call elimination discarded argument/lifetime effects without proof.
The fixes retain the original source range, distinguish lexical cells from
explicit class construction carriers, and remove only synthetic factories or
pure copies. Remaining constructor calls are effects even with an instance plan.
The first closure repair was too broad for explicit class-cell carriers; the
existing declared-global class-cell regressions caught that mistake. Selection
now uses the actual binding kind, not a special-case source spelling.

Annotation captures also replaced a source dictionary-first lookup with a
plain cell load. Preserving the effective lookup exposed a second bypass when
the source name shared a cell storage spelling. Explicit source lookup now wins
over storage aliases. Class-local type aliases additionally use the original
native child's complete freevar inventory, not transformed lexical inference,
to distinguish dictionary-or-global from dictionary-or-cell lookup. Native
code/projection validation is unchanged.

The first new annotation oracle incorrectly expected a declared class-local
name to retain an outer cell; stock CPython failed that assertion too. Native
inspection showed the exact dictionary-or-global versus dictionary-or-cell
distinction. Corrected independent dictionary-insertion and declared-type-alias
fixtures pass both stock controls and fail all four compiled/entry controls on
checkpoint 43, log SHA-256
`2be68611fe2636ae405babbfb32e836f18a6bfb2f11d2eff48fc45cbd3825555`.
The source-derived alias inventory test also fails before its repair, log
SHA-256 `cee38ca89468025fc824fe767c2deef247f5076e0a98f8ed5841427ec2528ff8`.
This distinction prevents treating an incorrect oracle as a runtime regression.

Several old structural assertions described previous representations. Updated
tests keep their original source scenarios and inspect real native owner
registries, raw `CellObject` loads, resolved local versus preserved operands,
and explicit `Take`/`Del` retirement. A second review corrected overly broad
new test premises without changing production ownership. The final lowerer
suite passes **535 / 535**, **5.32 seconds** (27.30 seconds with compilation),
log SHA-256 `af2b2eaf36a0a186ffa17f3a5c077cc3665d621368aa591114d28f8f41b2e00a`.
The generator effect-preservation repair passes the JIT library **821 / 821**,
**17.56 seconds** (75.41 seconds with compilation), log SHA-256
`8c797327b6580ce76fff57082a6527c7cc2fe3db4fb3a2447bbcffd206a44c2a`.
The final annotation lookup edits also pass all **821** JIT tests in **17.51
seconds** (52.35 seconds with compilation), log SHA-256
`4afc6b3a2c881e051daac498bba32184812c96df5dcd9a8b398089af93be9db5`.
The test-inclusive Cargo check passes in **5.14 seconds**.

Pytest batches now have a fixed four-node ceiling. Integration fixtures select
the worker's actual collected case/mode pairs without altering their reviewed
analysis source set, dependencies, policy, native witnesses or timeouts.
All **11** new scheduling controls fail before and pass after the change;
the real-collection control demonstrates the old redundant whole-cohort work.
Passing log SHA-256
`b0d4560efaa37c22990d7472d772c28a935b4efbaa7aab00c56997ac6b307cf1`.
These mocked scheduling controls are not strict admission evidence.

Both stack-capture entrypoints now explicitly load source-tree
`Tools/gdb/libpython.py` for out-of-tree builds, without weakening GDB auto-load
policy. The new workflow regression fails before; all **7** workflow controls
pass afterward in **7.02 seconds**, log SHA-256
`5024dc52e23eb25930d682a18208805dbceea38e131b3c3cb662105bf923c77a`.
A separate real capture reconstructs the selected native interpreter's Python
probe frame, stack log SHA-256
`80c20d9819575dc42ec252a277911f6056a9f3009170b3cd342546ce5abf8a11`.

The actual checkpoint44 extension rebuild completed in **38.56 seconds**,
SHA-256 `4bd9ac8bae3dd5b9ebae99c3736dd1eb1b30d8b534b8f1c5abc5f839f2d5dfae`,
source fingerprint
`c91af87180339884492280513094269439369397e2ee40d52858d0d96be0be0e`.
Actual import matches unchanged native20b/checker20. Its fixed-artifact replay
passes **69** and fails **2** tests in **404.72 seconds** (408.70 seconds with
provenance capture), log SHA-256
`47d88ef014c718fdcc3a27abc3be064a03396c42558ce307c3d50c39c16e3dfa`.
Both failures are the separate decorated-class import described below. Every
new native/compiled/entry annotation, alias, closure and original frame-cleanup
case passes. Inputs remain unchanged throughout the replay.

#### Decorated class frame ranges, checkpoint 45 (August 23, 2026 PDT)

The broader annotation replay exposed a class lifetime projection using the
full authenticated declaration range (including decorators) where the original
native code catalog requires its header/body range. The existing decorated
class/provider structural test now checks both the selected and resolved frame
range against that native unit; it fails before the producer fix, log SHA-256
`055f0bb41cca320a78779f15d5aaefc65292c4334cd61809ca4e8691d77ff56a`.
The producer now takes the native range directly, retaining the complete
declaration identity separately. Runtime equality checks stay strict; their
diagnostic now includes the qualified source and the differing frame/range.
Cache generation44 prevents reuse of the old frame projection.

All **535** lowerer tests pass after the repair, gate **24.17 seconds**, log
SHA-256 `35d1857157acf06086c703a9839306118320f13fe1ab1448076aeba44585cd60`.
The matched extension rebuild takes **35.04 seconds**, SHA-256
`d251cfa3d60b4c6ecdfab766d61c2d06b44730dfdb90ef830080ddf012ca48d3`,
source fingerprint
`9085b34c60ec612ce7abbae3f24dd50591ecf9a523d4fb8be3d6bad44804f1aa`.
Its actual native20b/checker20 import remains verified. Both decorated-class
runtime paths pass in **17.91 seconds** (21.78 seconds with provenance capture),
log SHA-256 `423365376c097c681fbd0993d34259a98854aec94285bfcb69fb9a1156e425f2`.
The final five legacy wrappers now have matching original integration
validators: all **12** native/compiled/entry checks pass in **40.50 seconds**
(44.46 seconds with provenance capture), log SHA-256
`0ca1229c7edaab8713ce75db5c186cb2db0dcb36bee2a32abe36af2cfaa4327b`.
Only the explicit strict rejection expectations changed; benchmark/admission
policy and original source bodies did not. Both replays retain unchanged
runtime and test inputs.

Native21 preflight stopped before source/build/selection changes because it
compared a guest VirtioFS inode from before the VM restarted with the new
guest inode. Current shared mount, all5,544 source hashes, generation, fstab,
selected runtime and selection still match. A separate read-only host/guest
metadata and boot review records this boundary; no historical host inode
equality is claimed. Promotion requires an explicit reviewed current-identity
receipt and must retain the current boot identity across later phases. The
failed preflight remains evidence, not a changed source or successful gate.
The full compatibility gate, native21/native22 integration and fixed-suite
performance remain pending; no completed contract or speedup is claimed.

#### Native source-parent promotion (August 23, 2026 PDT)

The explicitly reviewed cross-reboot preflight passes, receipt SHA-256
`72888b5bc682c48dd6cf3374d6363dd15e8ff1ad8241f8a96209a628eee8202d`.
It independently discovers 69 existing kernel tests and 352 native tests,
preserves the older inode observation, and pins the current boot throughout
the transaction. The authored0035 source-parent patch changes exactly ten
native files; all **5,544** files match the canonical patch replay, with no
generated-case changes. New native generation:
`ceee2ab5a75f6e9cb4c4518a57868fb0b89ffadb68665ca4c285c17b9b9193d1`.

The unselected development build succeeds in **33.68 seconds** and its actual
no-LD startup reports the expected six-word/48-byte scope ABI. The first
focused gate runs **zero** tests: including `pycore_interpframe.h` exposes
CPython internal unused-parameter warnings under the probe's `-Wextra -Werror`.
A diagnostic push/pop suppresses that warning only while including the
internal header; warnings remain errors in the probe itself. A separate,
explicit test-only continuation preserves the failed gate and reuses the
byte-identical native build. Native source, generation and selection do not
change during that repair.

All **20 / 20** source-parent controls then pass without skips in **2.38 seconds**,
log SHA-256 `791ae047c6c1f8d83ebf431a217b9027828d1754cd7ccfa6223452618cac47aa`.
The complete kernel passes **89 / 89** without skips under `-X dev` in
**3.00 seconds**, log SHA-256
`2bd132b032da5012d125c3f4da0f29bbf3a5f480150285e77e6e85fb9861a87c`.
This is the canonical nondebug development build, not `Py_STACKREF_DEBUG`
coverage.

The optimized PGO/LTO build then succeeds in **466.58 seconds**, using the
unchanged training workload. Its separate acceptance gate passes **20 / 20**
source-parent, **89 / 89** kernel, **89 / 89** `-X dev` kernel and **372 / 372**
complete native tests, all without skips. The CPython regression run expands
45 selectors into **49 files**, reports **5,334 tests / 74 skips**, and succeeds
in **58.84 seconds**. The C++ scope ABI, unchanged managed-generator layout and
all **5,544** source entries match the canonical replay. Gate receipt SHA-256:
`4ac283b3325ce8fa77d0d503afc425f917abf186d0833fb3960411a2473b5362`.

Following an independent gate review, native21 is selected at
**2026-08-23 07:51:15 PDT** and the repo venv is recreated. Its actual no-LD
executable/library identity matches the selected build. Ready receipt SHA-256:
`32709183333ebfe55878283a4ced6f918f1301701e32d25771954669cb13c75b`.
The older native20b build remains preserved. Selection does not stage an
extension or certify the offline checker. The Rust scope integration and
three structured/deopt tests are applied; the first test-target check fails
only on two missing PyO3 type qualifications in the new fixture. That fixture
is corrected and the check passes in **9.19 seconds**. Native22 reference-token
handoff, actual `Py_STACKREF_DEBUG`, complete runtime compatibility and fixed
full-suite measurements remain pending.

#### Offline exporter identity and builtin-field controls

The tests-first CLI run passes **20** selected tests and fails the new
selected-source alias test at the actual `__main__` dependency-identity
validation (**408.26 seconds** including the gate). The unselected `.pyi`
precedence control passes. Log SHA-256:
`5a2dbed8bfb871baa7050985ca8f8eb86f53d105b2ee65e21c1ca4bc1e151b63`.
This diagnostic checker has no production READY receipt; the previous checker
is archived unchanged.

An initial FastAPI diagnostic mistakenly used the repository venv, exposing
unresolved imports. It is retained as a wrong-environment run, not benchmark
evidence. The corrected replay uses the exact benchmark-venv executable from
the fixed 97-driver receipt, preserving its venv path and all source inputs.
It has no unresolved-import diagnostics and reproduces **eight** invalid
builtin-object field rows across `pydantic.main.BaseModel` and `__main__.Item`
before canonical validation rejects publication (**22.36 seconds**). Receipt
SHA-256: `7e1e5e69bc30451d930d9fc3c366d0ced255a7ccd317e4166276ff42082135d0`.
No benchmark algorithm, policy, or dependency selection changed.

The two new Pydantic unit controls initially fail before their intended
field-ownership assertions: one does not recognize the fixture's framework
base; the other lacks an expected field. These are not semantic RED
controls. The fixture omitted the real framework's `dataclass_transform`
declaration. The first upstream-test build costs **277.79 seconds** including
compilation, while the two tests themselves take **0.18 seconds**. Log SHA-256:
`54b8ceffda3b117c8435199b5a3ceb50c27c19132c16f75293c2b2a8f52f7c93`.

After correcting only that stub fixture, both controls fail at the intended
canonical-validation boundary: a field's declaring class is outside its logical
inheritance. The unchanged pre-repair implementation takes **0.43 seconds** for
the two tests (**16.20 seconds** including compilation). Log SHA-256:
`e6a91ee80370f670759ff83353549dc978bca675c38766a8b69bb93eae6c7645`.

The first production exporter21 build succeeds in **121.07 seconds**. Its full
CLI gate reports **34 passed / 2 failed** in **86.15 seconds** (**115.89 seconds**
including the gate), log SHA-256
`9c9ab071c099a30c1cfd7ae286583602de4795abe51f99ceae5623f71469d11c`.
One failure exposes an invalid test fixture: an unconditional module-level
raise makes the imported class `Never`. The other is a real regression in an
unchanged test: explicitly binding ordinary glob-discovered dependencies bypasses
new shadowing stubs. The repair narrows binding to actual strict sources, using
the new public `soac_source::has_strict_future` selector shared with the
exporter. This selects analysis behavior only; it cannot authorize execution.
The existing transitive-shadow-stub regression test is unchanged.

The alias fixture now uses file-writing import markers instead of an
unconditional raise. With the archived pre-repair checker and the same selected
native21 runtime, both corrected cases fail: `__main__` at dependency identity
(**3.14 seconds**), and `entry_alias` at unresolved import (**3.19 seconds**).
Neither initializer runs, no deployment is published, and source/runtime
identities remain unchanged. Receipt SHA-256:
`a008ab6776ca22d37dff39622d65b4ca43fcc946b3e35131722d64d6401588aa`.
The first production binary and failed gate are preserved. The revised build
then catches a missing direct CLI dependency on `soac_source`; that manifest
edge and its generated lockfile are being corrected before repeating the gate.
No after-repair CLI or fixed FastAPI success is claimed yet.

#### Source-parent structured gate after integration

The first three-test scope run passes deopt reuse and invalid-admission tests
but fails the suspend/terminal structure test. Its oracle initially mistakes
terminal PEP 479 conversion for a source callback. The corrected oracle uses
the typed `RuntimeName::Pep479Exception` constant and actual data relocation,
not rendered IR or an assumed helper address. It then exposes a genuine
invalid-ABI path: strict exception dispatch without a Python exception could
release owners while still linked. That path now traps before cleanup; ordinary
exception propagation is unchanged.

After this change, test-target checking passes in **7.18 seconds** and all
**3 / 3** source-parent tests pass in **2.79 seconds** (**50.68 seconds** including
compilation), log SHA-256
`14e52102a92f6bb1a62c1e8b2d411cd8ac940709ac1acd44e39afef1336adbb1`.
The existing unlinked deopt allocation-error cleanup control also passes
(**1.10 seconds** gate), log SHA-256
`7cd0c546f5fa77abc8463cb4020acebbb5dd71f86995386b22fa41cf3c87ee83`.
The shared source crate tests pass in **13.23 seconds**, log SHA-256
`a71d613952c549caa06a4458a151c6ab2497d1ecc8caca397203dbd84afeb091`.
These are compiler/structure checks, not a substitute for the pending matched
extension runtime replay, complete compatibility gate, native22 ownership
integration, or fixed full-suite performance measurements.

#### Locked upstream checker validation

With the direct shared-source dependency declared, the revised normal checker
build succeeds in **60.59 seconds**. Its full CLI suite passes **36 / 36**
(**86.83 seconds** tests / **112.89 seconds** gate); upstream project tests pass
**144 / 144**, including both corrected Pydantic controls (**15.43 seconds**
tests / **164.76 seconds** gate); both focused resolver tests pass
(**70.69 seconds** gate including compilation). CLI log SHA-256:
`5bb3d7725c5a93fa7c48be7f68a28c1bdff88cd7e0784c38ccb2d5adb262a826`.
Project log SHA-256:
`1c15c6a0100e26ced087ee90483e98e87c0f6c175c4d20795b4f7f49ecb8ec9c`.

The final source-integrity gate correctly refuses READY. The raw upstream
Cargo test commands were not locked and changed the canonical `Cargo.lock`.
Every other source file, the marker, and the file/directory set are unchanged.
The failed generation and normal binary are preserved; the cache is not
silently repaired, and the passing tests are not called a certified build.

The durable repair adds `just ty --test-upstream` for the two maintained
upstream libraries. It uses `--locked`, the existing verified configuration,
the shared serial checker target, and the same before/after canonical source
verification as normal checker builds. The existing runner test family covers
both library routes and the CLI route, Cargo error propagation, post-test lock
tampering, and refusal to execute the normal checker binary after tests.
The new routes fail before implementation (**6 failed / 3 passed**), then the
whole tooling family passes **37 / 37** after implementation and path-specific
integrity diagnostics (**0.16 seconds** for the preceding 36-case run;
**1.27 seconds** for the final wrapper gate). Final tooling log SHA-256:
`1ce1bfc2ae72629a4ddad2c4c42090f203b9f35059c9bdcea47054a284b30414`.

Lock generation occurs in a separate mutable review copy. An offline
`cargo update --workspace` reproduces unrelated cached-version downgrades, so
that output is rejected. The normal workspace refresh preserves **all existing
package versions** and adds the shared SOAC packages and their required
dependencies. The generated upstream lock patch has SHA-256
`553942d623a019a38b64df72a36822255e60febb49295271f22a952b662fa068`;
its generating command and package delta are retained in
`work/strict-ty-upstream-lock-review/workspace-refresh-online.json`.
This generated patch must remain in the separate generated-artifact commit.
The fresh canonical generation is
`f1540f8827209c2cb3cf344015be547a5720848a65bee7232bc80f31b4ed0b09`.
Its new normal build, locked upstream/CLI gates, fixed FastAPI replay and
matched runtime checks remain pending; there is still no performance result.

#### Certified checker22 and fixed FastAPI replay — 2026-08-23 PDT

The fresh locked generation passes its normal build (**120.99 seconds**), all
**36 CLI tests** (**146.00 seconds** gate), all **144 upstream project tests**
(**197.32 seconds** gate), and both focused resolver tests (**67.60 seconds**
gate). Canonical checker source verification then passes without changing the
lockfile or any other prepared input. READY SHA-256:
`0ff7b6fcc318ba0b928c55ac2f41d9dd6b56bb8a8aba3a50d8637d5cfaa5b2bd`;
normal binary SHA-256:
`98634650d8ac258ff00fef15315a6512c0baa4c3a4d6736ef24200924c0b7a1d`.

The actual fixed FastAPI source, policy, source manifest, benchmark venv and
native20 interpreter from the original 97-driver analysis are replayed unchanged
with this normal checker. Analysis now succeeds and publishes authenticated
`__main__` artifacts, whereas the retained before run rejected eight incorrectly
source-owned `builtins.object` fields. The selected native21 interpreter is not
substituted for the fixed benchmark interpreter. After-replay evidence SHA-256:
`c5939a96bb879412a0c86d7d9bff9d16c9e0348cf4a33a789a6b94ee65857b4c`;
log SHA-256:
`d61c9fbc19a611162d5ac7f097db4b1d76157e8359cfd5acef1b00b34c6efd3e`.
The **28.30-second** analysis gate is not a runtime or benchmark measurement.
It does not certify the remaining rejected drivers or framework execution.

Before these stages, shared storage falls below the 10 GiB build-start reserve.
Explicitly approved removal of only the two Rust incremental-cache directories
restores free space from **10.08 GB to 26.91 GB**. Selected native binaries,
checker binaries (including before/failed-run archives), source pins, source
snapshots and validation evidence are retained; their recorded runtime/pin
hashes remain unchanged. No disk guard is weakened.

#### Actual StackRef-debug bootstrap failure and ordinary frame controls

An explicit `stackref-debug` build mode now owns
`CPPFLAGS=-DPy_STACKREF_DEBUG=1`, configures pydebug without PGO/LTO, and records
a separate versioned actual-runtime proof only after native readiness succeeds.
Existing nondebug provenance is unchanged. The proof requires actual executable,
configured source/build, mapped libpython, debug-only exports and configuration;
`Py_DEBUG` or `-X dev` alone is not accepted. Internal-header C/C++ probes now
forward the actual configured CPPFLAGS. The tooling test family passes
**73 / 73** (**0.21 seconds** tests), log SHA-256
`cff40ecb14775332748e4a6f5c07745f376f06c95ae5f381ccbd09de9fa19bda`.
Those tests are not evidence that a debug interpreter built successfully.

The real unselected native21 diagnostic build fails after **34.50 seconds**
while bootstrapping ordinary CPython, before any transformed strict module.
Its `_bootstrap_python` reports an invalid StackRef close at `ceval.c:1958`.
No debug READY is published and the selected optimized runtime is unchanged.
Build log SHA-256:
`ac7fde4620912b16d27e5586bd737bc751030a0679e974c3041060ac434364a1`.
A native GDB trace establishes the ownership error in the pinned source:
`_PyFrame_Copy` consumes source code handle **37264** and creates destination
handle **37280**; `take_ownership` then overwrites the destination with DUP
handle **37292**, leaving the source handle stale. `clear_thread_frame` closes
that stale source handle. GDB's own exit status is not a passing control.

Three focused controls are added to the existing native frame/generator test
family before the source correction. They exercise escaped return frames,
traceback frames and closed-generator frames using freshly compiled mortal
code. They verify retained locals, exactly-once payload release on `frame.clear`,
and code release only with the final frame, without SOAC, a synthetic frame or
`gc.collect()`. All three pass on the selected optimized native21 runtime
(**0.052 seconds** tests), log SHA-256:
`5cc455574d75042c1b6841c8132f31afaf2671b2146c8f0f0d36a47bcd78d110`.
They have not reached their bodies on the failed debug runtime. The narrow
source-handle restoration fix, its real debug after-proof, native22 integration,
matched strict runtime tests and full performance protocol remain pending.

#### Matched source46 runtime after-proof — 2026-08-23 09:23 PDT

The source46 extension builds in **47.09 seconds** and its actual import is
verified against selected native21 and certified checker22. Extension SHA-256:
`8812fb470f959ff874c99d826c1234619b7a20666db0405c37616785bb6525ce`;
matched READY SHA-256:
`cf942b2ad3c871b6438559f3b1950b0675b3dde70a1e094d14af9a3dcb76988d`.
The focused maintained runtime cohort passes **82 / 82** in **408.17 seconds**
(**412.11 seconds** including provenance capture), with runtime and test inputs
unchanged. Log SHA-256:
`bbbbc66fd5e65d7e0d3eea28ece5a4ad56e809c9574f2ef8250807321acd16b2`.

Coverage includes real C-slot and Python-iterator exhaustion/errors, explicit
`next`, retained current source locals, delegated throws, iterator acquisition
and borrowed reads, selected return-check teardown, and active unannotated
functions whose code is replaced. Both compiled and forced-entry modes use the
genuine authenticated offline publication path. This closes the source-parent
lifetime defect without an extra loop traceback or a fake native PC. It does
not close the independent native argument/reference handoff, full-gate,
ordinary debug-bootstrap, or performance obligations.

#### Ordinary GC observations and source-frame inventory gap — 2026-08-23 09:51 PDT

The matched native21 runtime completes **16 ordinary-only GC observations**
in **0.36 seconds**, with source, interpreter, mapped libpython and selection
unchanged. Observation artifact SHA-256:
`f76215136169ee65246bfd3d8d18a783d000d031241509d22195c4412e5fa8d9`.
No SOAC extension is loaded. The four cases cover ignored `GeneratorExit`,
an async-generator finalizer hook that returns without closing, a never-started
coroutine, and a completed generator, each with/without an externally retained
source frame and two allocation-generation setups.

Retained suspended/created frames do not prevent the cycle's weak references
from being cleared during collection in these observations. Retained completed
frames keep the weak witnesses live until release. **Weak-reference clearing
is not physical deallocation, and `__del__` notifications are not allocator
events.** The oracle deliberately avoids dereferencing frame maps after their
weak witnesses clear; it proves neither stale map pointers nor readable maps.
A separate allocation/clear probe and actual admitted native22 counterparts
remain necessary. No map pins or new GC edges are justified by this result.

The source46 legacy-replacement replay exposes a real frame-inventory gap in
both compiled and forced-entry modes for `dictcomp_temp_collision`. The actual
original native frame includes its eager-comprehension iteration target `__`
as `CO_FAST_LOCAL` (32), whereas the parser-owned enclosing inventory excludes
that target and SOAC lowers the comprehension into a private helper. The
assignment-expression binding `c` is not the missing row. A minimal maintained
case now separates the loop target from a containing assignment expression.
The existing missing-projection refusal is preserved; an unsupported slot
does not become a guessed unbound row or a name-based native owner.

The diagnostic now identifies the exact missing native name and locals-plus
index. `cargo check -p soac_jit --tests` passes in **9.17 seconds**, log SHA-256
`1169eae39e0479263a244b0cbb37fdc66232fb731e8e87b0f7149367ac834d67`.
This diagnostic is not in the still-running source46 extension. The native
inline-comprehension save/clear/restore producer and resolved source-primary
mapping, the new minimal runtime replay, the remaining replacement cohort and
the next full gate are still pending.

#### Source47 gate and native-reference composition — 2026-08-23 10:08 PDT

The source46 replacement cohort completes with **163 passed / 2 failed** out
of 165 nodes (**1307.44 seconds** tests; **1311.34 seconds** gate). Both
failures are the compiled/entry `dictcomp_temp_collision` frame-inventory
defect, not the assignment-expression binding. Log SHA-256:
`c0d579388a4d0868393dc8643d9507d1c726c1fa2c9f19a482f5aa21970f827f`.
The new minimal maintained loop-target/containing-walrus case independently
fails in both modes (**268.84 seconds** tests), log SHA-256:
`bbfab5f1132aa1c09452f1a87f4ced02574c750f55b6b19a54cbcc4716999084`.
These are retained red controls; no unbound-slot or raw-cell substitute is
introduced.

Only passing replacement coverage is consolidated: **50 duplicate legacy
wrappers** are removed, while all 12 unique cases, original source modules,
validators, catalog routes and shared helpers remain. The two wrappers for the
failing dictionary-comprehension source remain, including their still-used
helper. This is not a 52-test deletion or a claim that every replacement passed.

The source47 extension, including the precise missing-slot diagnostic, builds
in **34.98 seconds** and its actual import matches native21/checker22.
Extension SHA-256:
`d31120d13f6d1b59aaef5f1f27332579ca2f7dae9dbf0f39eac5caffb78c5d1c`;
READY SHA-256:
`57a5ca2db70b2755990bb701b871dd87189399cd2d70060155944417ec3a66f3`.
An actual `just test-all` is running against frozen live inputs. It has reached
Python integration tests and has reported failures; no final gate result is
claimed here. The independent frame-copy repair is not applied during this run.

All 12 frozen native-reference Rust prerequisite packets are composed into an
ignored mirror of the current 41-file input set, without live changes or
compilation. Explicit context ports preserve the newer dictionary-first class
branch, every source-parent/kernel test, and the decoder's expanded exports and
receipt definitions. Original packets and failed dry-run logs are retained;
only their reviewed context differs in the derived composition. This data,
representation and lifetime work does not authorize a native-reference body.
The source-operation wire3 overlay, real native22 builds, checked token-aware
body integration, compatibility after-proofs and performance measurements are
still required.

#### Ordinary physical-allocation probe — 2026-08-23 10:05 PDT

The isolated C probe builds in **0.54 seconds** and executes all **16 cases**
in **0.62 seconds** on the exact selected native21 executable and mapped
libpython. It records **276 native events**, no buffer overflow, and **zero
differences** from the original oracle's event/checkpoint observations. Prepared
native sources, selected binaries, packet sources and original observations
remain unchanged. Result SHA-256:
`213f32594ccb1bbe0473372e879fa70d4c9f98540cdc2bc6285e691608754681`.

The private diagnostic type delegates ordinary dict traversal/clear and calls
the original allocation mate exactly once; its registry retains scalar
generation/address identities, not source objects. Actual `tp_free` return is
separate from weakref clearing, Python finalization and dict watcher
deallocation notifications. Public frame-map getters are skipped for a retired
allocation generation even if its address is reused.

Retained `ignored_exit` frames contain original but already-freed map addresses
after collection, while their function's corresponding fields are NULL.
Retained completed-frame controls can read their maps and return to the same
refcounts after each getter. This is ordinary code on **SOAC-patched native21**,
not yet proof about unmodified CPython or an admitted native22 body. The native
patch provenance and other cases are under review. Neither reproducing a
use-after-free nor adding unexplained persistent map owners is an acceptable
implementation inference from this observation.

#### Composed Rust type check and gate diagnostics — 2026-08-23 10:40 PDT

The ignored native-reference composition now passes the actual guest
`cargo check --locked -p soac_jit --tests` in **31.01 seconds**. It includes
the 12 prerequisite packets, source-operation wire3, the reviewed terminal
map-view correction, and narrow compiler fixes. Those fixes preserve the
corrupt-wire tests, make callback-free raw-field borrows explicit, and do not
create body authority. The preceding checks retained their failures: 28
errors, then two test-helper lifetime errors, then three implicit-autoref
errors. The passing log SHA-256 is
`0d8e08ff3913c2491adaf902d4b0078a827650e830cf6ee511a8bd205b993d98`.
The derived workspace has an explicit independent Cargo target; its inputs,
the live source47 gate inputs, and selected native21 binaries are unchanged.
This is **type-check evidence, not native22 execution or a token-body result**.

The still-running source47 full gate reports **824 passing `soac_jit` tests**
and **238 passing / 10 failing `soac_opt` tests**. The optimizer failures cover
one obsolete source-loop recognizer, class-field catalog selection, and
constructor/iterator field propagation. Python integration has additional
behavioral failures and 300-second worker timeouts; no completed gate count
or success is claimed. A captured timeout stack shows the offline checker
actively hashing source bytes while exporting class references, not a lock
wait. A separate, uncompiled per-export source-digest memoization draft keeps
the database borrow immutable and adds same-size dependency-edit invalidation
coverage. Its speed effect remains **unmeasured**; source verification and the
fixed analysis source set are not relaxed.

The physical-allocation probe audit establishes that all 12 map allocations
in six retained suspended/created cases retire before the first post-GC frame
checkpoint. It skips 24 unsafe getter opportunities; the two retained
terminal controls perform eight safe original-map getters. A source-only
comparison traces the relevant retention/GC mechanism to official CPython
`d73634935cb9ce00a57dcacbd2e56371e4c18451`; no unmodified upstream executable
was built or run. The reviewed reference-Finish overlay unpublishes borrowed
maps before every successful terminal path, including unique and never-entered
frames, and acquires no replacement map owners. The legacy object-owner API
is preserved. New native phase/GC/getter controls are unrun pending native22;
active/resumed map support and real body cleanup still require integration.

Full-suite timings, previous-strict-SOAC deltas, and executable native22 body
coverage remain pending. None of these checks changes the performance verdict.

### Isolated optimizer and checker follow-up — 2026-08-23 10:56 PDT

The actual isolated optimizer run now has **250 passing / 3 failing tests**
(`work/native22-rust-static-check05/optimizer-tests-result.json`, 77.16 seconds
including build, 1.54 seconds in tests). The catalog repair tracks immutable
literal values through validated operand moves; it does not turn the consumed
operand slot into a live owner. The new structured negative control removes
the operand role and requires catalog rejection. Seven of the original ten
failures are resolved in this candidate; no fix is yet applied to the frozen
source47 gate. The three remaining failures concern constructor field
bindings and iterator field scalarization.

A second actual diagnostic run shows constructor values passing through
`Store operand <- Load local` followed by `SetAttr(..., TakeOperand(operand))`.
The old collector accepts only `Load`. A sound repair must retain the value
relation to a still-live primary and invalidate it when that primary moves or
changes; reloading the consumed operand or adding a hidden value owner is not
an acceptable shortcut. No constructor repair is claimed yet.

The digest-cache candidate passes its new same-size-edit invalidation control
and the complete upstream `ty_project` suite: **145 passed**, 19.10 seconds in
tests (71.62 seconds for the locked runner). Receipts are under
`work/ty-export-digest-reuse-check/`; the full-suite log digest is
`f9a543fa45a88fba67a5653dc2c56dfbf46563f6e73adc0a63fb4ca0f585dc0f`.
This uses the unchanged normal locked runner in an isolated source generation
and separate target with test debug information disabled. It is not a normal
checker READY, a fixed-CLI timing comparison, or pyperformance evidence. Live
checker/native/source inputs remain unchanged. Full runtime compatibility and
all performance acceptance measurements are still pending.

### Native read-to-emitter composition — 2026-08-23 11:41 PDT

The isolated native-reference candidate now connects the existing privately
authenticated original-code catalogue to canonical read selection, an owned
typed-IR snapshot, and the existing process JIT. The original shared template
is unchanged. Conflicting preselected read operations and mismatched physical
parameter primaries are explicit rejections. The first emitter subset handles
parameter reads and one-time compiler-operand moves; it refuses constants,
cells, source stores, calls, joins, and suspension. It emits native opaque-token
Dup/Borrow/Take/MakeHeapSafe calls rather than converting through owned Python
objects. Raw-object entry accessors reject the distinct token ABI.

This composition passes actual guest `cargo check --locked -p soac_jit --tests`
in **21.06 seconds**, with log SHA-256
`d815a1db5e7d02d9566a290fa807088c179ccea72d306ff86e030d6764ef261c`
(`work/native22-rust-static-check14/check-result.json`). Its earlier emitter
and argument-handoff-only composition passed in 29.05 seconds after eight
test-only compiler errors were corrected in a separate overlay. No native22
binary has executed these functions. Registration remains structurally
unavailable: the executable-interval type is still uninhabited. Recursion,
pending events, observer refusal, borrowed checked boundaries, and actual
terminal owner cleanup must be joined before that changes.

The original source-continuation sidecar also typechecks. Its **two core** and
**nine lowerer** structured tests pass, covering exact parser identity, shared
finally alternatives, with-item ownership, error-prefix splitting, and the
real generator resume ABI. The initial generator assertion incorrectly assumed
a new helper function ID; production retains the original callable ID and
distinguishes the resume binding. The corrected control verifies both facts
and explicit rejection of unrepresented suspended context. The new optimizer
inline control currently fails admission because its actual lowered fixture
has 32 blocks against the existing 16-block limit; the limit was not raised.
Cache/inline controls and native execution remain pending.

All source47 full-gate inputs and selected native21 binaries remain frozen.
The full gate is still running and failing; no final result is claimed. The
isolated target accumulated 4.68 GB of logical incremental cache, which was
removed with approval after confirming no Rust compiler was active. Sources,
binaries, dependency artifacts, logs, and receipts were preserved. Subsequent
isolated checks explicitly disable incremental caching. This is workflow
evidence, not benchmark data; the full-suite performance verdict is unchanged.

### Borrowed boundaries and continuation checks — 2026-08-23 11:57 PDT

The composed source15 candidate passes guest `cargo check --locked -p soac_jit
--tests` in **24.04 seconds** (log SHA-256
`fbc1ed53b61645c8e43d00a31ac3a3bc39df5af7b591674b6c238d754ebaf5ea`).
Successful native argument/return predicates now inspect borrowed token views;
they do not acquire argument, function, code, or map owners. Return rejection
marks the attempted check before error construction, publishes the result slot
Empty before closing it, and preserves the exact detached exception through
cleanup. Unfinalized nominal checks that need the old function-owning snapshot
are explicitly declined at cold admission. These native execution/ownership
controls remain **unrun** without the matching native22 binary.

The source-continuation cache round-trip/rebase control passed (**one test**,
0.02 seconds; source14 log
`5e0512d4faa18aeb57d27347805a2b15741683737aecd1537a1c40a682b964d3`).
The original finally-containing inline fixture now uses the production typed
guarded-call inliner, whose normal eligibility does not have the legacy
helper's 16-block cap. The actual source, generic fallback, target guard, and
complete callee CFG are retained; no block pruning or cap override was used.
A test-only undeclared crate import was corrected by inspecting the existing
typed source ranges, without adding a production API or dependency. The fully
qualified focused test passed (**one test**, 0.02 seconds; source16 log
`df01cb135d06f49ecd0b625bc5bdf96b5d2e8c129b0acc62eb5dad6a4adafee3`).
The complete isolated optimizer suite is now **251 passed / 3 failed**, 0.76
seconds in tests (2.00 seconds total; log
`5285dc738d4a67229b6c2595cb0e1cc71b5537b95ffea66f2d61d8010b897edc`).
The same three constructor/iterator operand-value propagation failures remain.

Source47's full compatibility gate continues against unchanged live inputs;
its failures are not covered by these isolated results. The native scope-wire4
producer and controls have been reviewed as source, but have not been compiled
or run. Entry interval review also identified missing recursion, initial-RESUME,
and observer-mutation barriers: linking a lifetime frame alone is not a native
body entry protocol. Source-body registration remains unavailable until those
semantics and complete error/terminal cleanup are implemented and exercised.
No strict-suite throughput or stock/previous-strict delta is claimed.

### Compiler preflight and checker candidate — 2026-08-23 12:27 PDT

The scope-wire4 compiler composition first failed C syntax checking because
seven `compile.c` call sites used `LOC`, an alias private to `codegen.c`.
Replacing those calls with the existing shared `SRC_LOCATION_FROM_AST` macro
preserves the location expressions. The corrected `compile.c`, `codegen.c`,
and `flowgraph.c` all pass `-fsyntax-only` under the recorded native21 GIL
configuration (**6.59 seconds**, log
`44b4d85f0670ba535c54cd78d1a0fd429d117c23b230b95abd8e165fbd1b0f5b`).
Both the failed receipt and correction are retained. This is compiler syntax
evidence, not a matching native22 build, generated-case check, or execution.

The global-class native-qualname correction passes guest Rust typechecking for
`soac_jit`, `soac_lowering`, and `soac_opt`, including test targets (**30.09
seconds**, source17 log
`a544d83d3de184327523824dd5c68109ea00f8379f3ba0f05cbde8150d845c43`).
It preserves signed lexical identity while comparing the lifetime association
with the actual native qualname. The new all-scope wire4 Rust packet composes
without overlapping earlier fixes, but its first check found one remaining
old `.exports` consumer. That consumer is being migrated to the same recipe's
mandatory class-actions subsection; absence is an error, not empty exports.
Matching-native behavioral controls remain pending.

The per-exporter source-digest reuse candidate built with the unchanged locked
checker runner and normal debug profile (**208.61 seconds**). Its real CLI
suite passes **36/36** (**388.26 seconds in tests**, 465.29 seconds overall;
log `6810fcc28b96cd1c92f1c57e624a53c3a633da78ca82fea2b9c709de30bd16fa`).
The corrected selected-source resolver invocation passes **2/2** (39.87 seconds
overall; log `cbfaeb70f92c46d8e97b2c44ee9dcb95695b6c423f7379f8b61544f4abb2ada1`).
An earlier guessed resolver filter selected zero tests; that successful process
exit is explicitly excluded from coverage. Together with the earlier 145-test
exporter run, these are correctness checks, not an offline-cost comparison or
strict-runtime performance result. The selected checker is unchanged.

The host-volume reserve stopped another isolated preparation before writes.
With approval, the completed canonical checker's rebuildable dependency and
incremental caches were removed, preserving its selected executable, 46
standalone executable/test artifacts, sources, and logs. The receipt records
29.93 GB of logical cache files and **15.05 GiB of actual recovered capacity**.
The guest's sparse disk shares this host backing volume; its apparent free
space was not used to justify duplicating caches. Full-gate inputs and selected
native binaries remain frozen while the Python gate continues.

### Native-entry preparation and exporter cost — 2026-08-23 13:05 PDT

The remaining class-export consumer now uses the mandatory class-actions
subsection. Source19 typechecks all three affected Rust crates including their
tests (**28.17 seconds**), and its six new pure scope-binding tests pass. A
broader canonical filter passes 43 tests but fails 12 native-backed fixtures
at the old selected compiler's two-part packet rather than the required
four-part packet. Those failures are preserved; they are not a schema4
execution result and the fixture was not weakened.

The first real emitted-reference-body join also typechecks (**30.04 seconds**,
source20 log `a48041265cd398a2343c70fdeaaf01ed6f1f8c04029539489a490d3939614011`).
It connects captured native binding, mandatory borrowed checks, one native
source-frame interval, and terminal primary cleanup. Three drafted tests use
actual emitted parameter reads, C and warmed VM calls, rejected values, and
failed-Begin cleanup. The matching native implementation and these tests are
still unbuilt/unrun; production installation is not enabled. In particular,
the existing zero unchecked-direct ID cannot be repurposed as checked native
entry authority. Native registration needs a separately validated association.

Named-expression lowering previously stored into a cell and reloaded that
cell for its expression result. A displaced value's finalizer can rebind it.
The new structured test fails on that lowering (**one failed test**, source21
log `a3859e255e3b2cf47aae8489dc94bed794d726a26a323434fccb2164bccf49c7`).
The repair copies the RHS into an operand before the actual Store and returns
that operand. Its first compile exposed an invalid bridge-IR conversion;
using the existing unresolved-name helper with synthetic metadata fixes it
without inventing a native source-load receipt. The test then passes
(source23 log `b5b4671f11f73e26495e2cee2016984e266a3f5436e8e3990b9c8011343b5a31`).
All three ordinary native controls also pass (**0.56 seconds in tests**):
success, a later-argument exception, and a consuming-call exception preserve
value identity, handled exceptions, explicit traceback retention, and
exactly-once finalization. Strict compiled/entry controls remain pending.

Two independently started, order-alternated pairs compared the normal debug
checker binaries on the same retained FastAPI project, interpreter, inputs,
policy and signing key. Current checker times were **29.8894 / 28.6527 seconds**;
digest-reuse candidate times were **20.1761 / 20.3910 seconds**. Medians are
**29.2711 versus 20.2836 seconds**, a **30.70% analysis-time reduction** in this
diagnostic. The full compatibility gate was running concurrently, so these
are not clean steady-state timings, suite-wide results, or strict-runtime
speedup evidence. The retained native20 benchmark interpreter was used for
both analyses; selected native21 was not substituted.

An initial exact-shard assertion failed as the checker build identity changed
254 vendored dependency resolution fingerprints. Independent recomputation
from the actual checker markers, exporter source fingerprint and every
recorded dependency source explains all differences. Type facts, source bytes,
dependency observations, policies, interpreter identity and all other
environment fields agree; each build repeats byte-identically. Published
artifacts were not modified or relabeled. Essential receipts are under
`work/ty-export-digest-fixed-fastapi-comparison/`, including the failed
byte-equivalence diagnostic and `provenance-result.json`. The canonical
checker and selected runtime remain unchanged. Stock/previous-strict/candidate
runtime ratios, native code-size deltas and final suite acceptance remain
unavailable.

### Full-gate failures and native prerequisites — 2026-08-23 14:03 PDT

The first source47 `just test-all` completed with unchanged source, native,
checker and extension inputs, but **failed** (12,824.71 seconds overall).
The Rust JIT suite passed **824 tests**; the optimizer suite passed **238**
and failed **10**. Python completed **603 passing batches / 193 failing
batches**, out of 796 batches covering 2,854 collected node IDs. These are
batch counts, not individual Python test counts. Failures include 300-second
timeouts, real semantic defects, and fixtures still assuming ordinary SOAC
admission. No timeout was raised or failed batch excluded. The full log is
`work/logs/strict-v21-fullgate47-first.log` (SHA-256
`efde2345293abdcc6a8730bdd107eb9661431cf01b88fc08a68ef614c4c99ba5`).

An isolated operand-fact/scalar-field candidate fixed four reproduced
structured failures and reached **257 optimizer passes / one failure**.
It is **on hold**, not accepted: replacing object fields with frame-lived
scalar owners while removing object-alias `Del` operations lacks a proved
last-owner retirement schedule. Observable surviving-payload reference
counts and finalizer timing are not covered by the approved field-release
order exception. Even the narrower analysis-only fact recovery can enable
that existing owning consumer; it needs a refusal before any scalar-slot
allocation or partial rewrite until original cleanup actions prove retirement.

The public native-source installer draft now typechecks both `soac_jit` and
the normal `soac_pyo3` dependency path, including tests (**24.18 seconds**;
source28 log `6db4204b4bcd63a3afde58e915ede578933cae7483e21ff3a1d5531b4fb36618`).
The first broader check exposed that the raw runtime dependency had only been
declared as a dev-dependency; focused JIT test-target checks had missed the
normal-library failure. Moving that existing dependency to normal dependencies
fixes the build graph without a lockfile dependency change. Actual matching
native22 link, installer execution, emitted-body counters and rollback failure
controls remain pending. A successful Rust check is not native-entry evidence.

The independent native frame-copy repair replayed correctly but its debug
build exposed an ordinary specialized Unicode owner/borrow ordering defect.
Two minimal cases (`path += suffix` and `path += path`) each reached their
warm loop and aborted while stealing a local owner with live operand borrows.
The first diagnostic used the bootstrap executable's unsupported `-c` form;
it is retained as a CLI failure, not a behavioral red test. The corrected
script-path invocation reproduced both native failures. Retiring the right
operand and closing the left borrow before stealing the target local repairs
both distinct and aliased operands without changing string results.

The maintained regeneration recipe succeeded. A transient preparation pin
precheck then failed; identical direct and `just`-wrapped Git checks showed
the expected resolved stage-zero pin. No index mutation or speculative pin
repair was performed. A checked resume succeeded, and a fresh complete
5,544-file upstream/patch replay matches the live source. Only `bytecodes.c`
and three generated case files changed from the frame-fixed generation.

The Unicode-repaired StackRef-debug interpreter compiled and bootstrapped.
All **five ordinary controls pass**: the two specialized Unicode cases plus
returned-frame, traceback-frame and closed-generator mortal-code lifetime
controls (**3.88 seconds**, log
`15175287ae6a68141cfbf478cfb104a410a301caae13a72a50cd379bcc7dda94`).
The build is nevertheless **not ready or selected**: its required
`_testinternalcapi` extension fails import with an undefined
`PyStackRef_UntagInt` symbol. That debug-only export issue must be repaired
before broader native/CXX/CPython validation. The selected native21 runtime
and extension47 remain unchanged. No benchmark acceptance or performance
improvement is claimed by these correctness diagnostics.

### Debug shutdown and ordinary ownership controls — 2026-08-23 14:34 PDT

The five missing StackRef-debug functions now have explicit `PyAPI_FUNC`
declarations. The before checks reproduced both the `_testinternalcapi` import
failure and all five absent dynamic symbols. A fresh, unselected debug build
then completed, and **seven ordinary controls passed** (the prior five plus
two export checks). This is a real `Py_STACKREF_DEBUG` build, not merely a
debug interpreter or `-X dev` invocation.

The broader native run reported **207 successful test bodies**, then aborted
at shutdown with 98 open StackRef records. Its overall result is **failure**,
not 207 passing tests. Family and method isolation identified one delegate-close
test. An ordinary `yield from` control, importing neither SOAC nor the native
probe, also completed its assertions and aborted at shutdown. A separate
diagnostic showed that captured `UnraisableHookArgs` were untracked while
their exception, traceback and capture list were tracked. Releasing the only
external capture owner did not collect the cycle; explicitly breaking its
internal list edge did. That deliberately cycle-breaking diagnostic is not a
successful regression test. A narrow fix and an unbroken external-owner/GC
regression are being validated.

The affected CPython run passed nine files and failed two. One failure was a
runner mistake: this pinned tree uses `test_str`, not `test_unicode`; no Unicode
suite coverage is claimed for the missing module. The other was an actual
`test_frame.test_overwrite_locals` abort when closing an ancestor fast-local
owner with a live descendant borrow. Two additional ordinary controls reproduce
that abort after proving actual borrowed-load bytecode, on normal and exception
paths. The existing frame-owned overwritten-locals tuple retains the Python
object, but the debug handle table does not represent that ownership transfer.
The proposed repair must preserve the tuple's reference operations and cleanup
order, keep child-borrow checks, and cover escaped frames and reentrant clear.

The production installer, owner identity and normal extension dependency path
also typecheck on the source29 base **without** the held owning-field optimizer
candidate. General same-activation scope projection is now composed on that
safe base. Broad library/test checks exposed obsolete test constructors and
one owning-borrow location comparison; these are being adapted to the explicit
IR API, preserving their original cases and assertions. Matching native22
execution, allocator-failure rollback, strict full-gate success and benchmark
acceptance remain pending. The selected native21 interpreter and extension47
are unchanged.

### GC repair and owning-field refusal — 2026-08-23 15:05 PDT

The unraisable-hook cycle is now repaired in canonical native patch 0050.
`PyStructSequence_New` produces an untracked object; the hook-argument factory
now tracks it only after all five fields are initialized and its error check
has succeeded. The regression releases the sole external capture owner and
requires ordinary cyclic GC to release the payload, without clearing an
internal edge. The before build fails that control and aborts at shutdown.
The repaired, unselected StackRef-debug build passes **8 ordinary controls**
and **208 native tests with clean process exit** (8.99 seconds; native log
`2e06c76a60e213aa80357e66fe73648143cca3bfa681e6a6e992229998ca7087`).
Six affected CPython files pass **484 tests, 17 skipped**: `test_sys`,
`test_exceptions`, `test_generators`, `test_gc`, `test_structseq`, and
`test_str`. The runner verifies those exact module files before execution.
These are native correctness results, not strict-JIT or performance evidence.

The owning-field candidate remains excluded. A fail-closed eligibility check
now precedes every full/partial virtual-object rewrite and materializer that
would move an owning field, until original retirement and source-frame owner
transfer have a joined proof. The test fixture initially panicked because its
inliner-generated instructions lacked IDs. Assigning missing IDs through the
production helper preserved the assertions and reached the actual defect:
**6 behavioral failures and 2 passes before the refusal; 9 passes after it**.
The after run took 60.58 seconds including compilation (log
`8c6af7131aa70b835a1d1cfcd32030d782dd4aa48cdc04b99ef08aa56f8216a1`).
No new proof-grant path or unsafe owner rewrite is retained. Broader library
and test-target checks passed before this final refusal patch; the complete
optimizer suite and actual matching-native installer tests remain pending.

The frame-overwrite regression also exposed two observer mistakes: unittest's
`assertRaises` clears traceback frames, and a retained frame's `f_back` retains
its caller even after `frame.clear()`. Corrected controls preserve these actual
lifetimes. The original 6-pass/2-fixture-failure nondebug result is retained;
the corrected run passes **all 8 nondebug controls**, while **all 8 debug
controls still abort at the original borrowed-owner overwrite**. Native patch
0051 leaves the existing overwritten-locals tuple's Python reference operations
unchanged and records its ownership transfer in the debug handle ledger. A
fresh 5,544-file replay matches the live patched source; its new debug build
succeeded. After-repair controls are pending at this checkpoint. The evidence
directory is `work/native21-overwrite-fixed-v2-v2`; its doubled suffix is an
unintended driver naming artifact, not a second run or a rewritten receipt.

Fullgate47 is still failed. Its Python outcome is **603 passing, 89 failing,
and 104 timed-out batches**, not 193 confirmed semantic failures; batch
membership does not establish how each contained test would finish. The gate
also has 824 passing JIT tests and 10 optimizer failures. The next checks must
separate obsolete ordinary-admission fixtures, setup/checker time, and actual
compiler defects without raising timeouts or excluding failed tests.

### Canonical generation checks — 2026-08-23 15:24 PDT

The frame-ledger repair's after run passes **16 ordinary controls** and
**216 native tests with clean shutdown** (9.81 seconds; native log
`f7f6d0ffb83f5a6aac2503d48fc45a818793d6364bf253f3d540c98cd2b913d7`).
The affected CPython run passes 11 files and fails one of 1,574 test cases:
`test_sys.SizeofTest.test_objecttypes` still expects a frame without the new
debug-only pointer (200 versus actual 208 bytes). Patch 0052 extends the existing
C layout probe with `offsetof(PyFrameObject, _f_frame_data)` and uses it in the
size assertion. This is a test-layout correction, not a runtime size override
or a skipped test; its matching-build after result remains pending.

The reviewed native22 source and 171 added native test methods are now composed
with the independent fixes. The full pre-regeneration inventory has 5,558
files. The first actual handler regeneration failed because the case analyzer
did not know that `_PySoacVMCall_IsRegisteredV1` is callback-free. Its complete
implementation only reads native identities, copies a C spec, and may detach
and free a C-only stale registration record; it never executes a body or calls
the registrar's callbacks. Patch 0053 declares only that query non-escaping.
A maintained test uses the real analyzer's structured effect properties for
three branch guards and retains rejection of `FinishV1` in those guards:
**1 failure before, 1 pass after** (logs `92f14c0eb6ad01c4c636fc0009ef837290c67b186e4b1f798e6adcba9afc869b`
and `99a24796443a1d07f66a424f8116a06a2079063af758e7d3c87801c191ad2ecd`).
This raw generator test used the retained selected native21 interpreter; it
is not native22 execution evidence.

The second actual regeneration then rejected a cached `stack_pointer` argument
at an escaping `FinishV1` call after the generator had spilled stack state.
That failure is preserved. The repair must read the actual published caller
stack, keep the body call escaping, and preserve every input's retirement and
exception-unwind order. No native22 interpreter has been built or selected at
this checkpoint; stale generated handlers are not accepted as a build input.

Canonical checker patches 0023 and 0024 are installed with the exact reviewed
pin. The normal executable was built through `just ty --debug-build -- --help`,
not copied from the earlier candidate. The build passed in **119.73 seconds**
(log `a17522b6a4d1449627192a57126a6bec88157032b8b1953af870dffba7a10bc8`),
with binary SHA `fba031c3df421f4ccaf22e3d7a11fc6c124620cbdcd96f4c7c90288b70643634`
and source generation `4488e43828725f63a444932e9f1b4a99721193d38f9caceb7d485387b81e9ac8`.
The previous canonical22 executable and receipts are preserved. Focused/full
semantic tests, signed-artifact identity verification, matching-native runtime
controls, and the fixed97 reanalysis are pending. No new publication count,
runtime speedup, or full-suite acceptance follows from this build.

The fullgate audit identifies **152 explicit failed node summaries** separately
from batch outcomes. Of these, 50 belong to already-drafted fixture migrations,
27 to rejected in-process admission, 21 to ordinary-profile/implementation
expectations, 53 to known scope/generator/argument-lifetime work, and one to a
genuine authenticated apply-codegen verifier failure. Only 62 of the 104
timeout captures show the exact exporter source-digest stack; other checker,
setup, compiler/I/O, and missing-child samples are not attributed to that fix.
The apply failure has a candidate missing LocalEnv join in the guarded list
setter; a structured before/after and the original authenticated runtime
fixture must verify that candidate before it is retained.

### Native22 build and first actual gates — 2026-08-23 16:03 PDT

Patch 0054 fixes the generated-case stack contract: all three escaping native
body completions read the published `frame->stackpointer`; the pre-consumption
expanded-call refusal uses `ERROR_NO_POP`. The new structured generator test
fails before and passes after, including four negative source mutations. All
**14 patch-workflow tests pass**. Actual canonical handler regeneration passes
in **19.03 seconds** (log
`7e2b43460989371d9b4a94d25b9b62e4a3e4a27736aec5c7ba3c10d2c5d971a5`),
and a fresh 5,558-file replay equals the live native source. Generated patch
SHA is `ad07d62264e7293e974f7f2c4d470b16f62f24c25df78c410d83b7f98d1fb595`;
the stale native21 handlers are no longer build inputs.

The first native22 debug build fails because the generated test interpreter
does not see the VM-call declarations. Patch 0055 moves their include from
`ceval.c` to shared `ceval.h`, without changing the ABI or generated handlers.
A fresh unselected debug build then passes in **31.04 seconds** (log
`0d389eaa8ef23884e6b137a79ee4526658ad1eb8e351bb0dd89be0461778f7b1`),
with source generation
`a3576f2208d4b421f979eb17cbb05d7df3de3c7dfa2db3d6163b4f5a2d0fcc29`.
The failed build, old selected native21 runtime, and both receipts are retained.

Its first native gate is **failed**, not ready: 345 tests execute, with two
failures and 14 errors. Eleven errors prevent the C probe families from
starting; the probe omitted its public reference header and then used the
core-program rather than core-extension build role. Isolated corrected probes
compile and load against the actual new library, without exporting hidden TLS
or embedding a replacement runtime. Their first execution exposes a separate
invalid test destructor: leaving a new exception from `PyCapsule` deallocation
correctly aborts CPython's debug check. The revised observer uses valid
unraisable callbacks and restores the exact incoming exception on return;
ordinary and SOAC controls run identical observers and retain all cleanup-order,
pending-error, and terminal-reentry assertions.

The metadata failures also separate real defects from incorrect expectations.
The producer snapshots FREE entries before native annotation codegen can add
implicit class-dictionary support. Proposed 0056 completes metadata from the
final actual maps, preserving all final-layout checks and ordinary bytecode.
The finally test must distinguish deferred constant-return payloads from
already evaluated values; a nested `global` also changes the module's same-name
store form. Expanded controls preserve both source scenarios and add native
instruction comparisons. These repairs are not yet built at this checkpoint.

The affected CPython gate passes **49 of 51 files**, with **5,494 test cases,
seven failures and 69 skips** (log
`e0e23baae603cf32ccc315a025b45dc68e027b2f8c56bd8647c68039f70aa740`).
One failure is the function-size assertion, which omits the native registration
pointer and source-owner ID; its generator assertion also contains a stale
local variable. Proposed 0057 corrects those test expectations. Six failures
are a real debug-runtime issue: no-GIL watchdog/fatal dumps reach debug StackRef
lookup through `SafeGetCode`, which requires a current interpreter and aborts.
The unchanged verbose rerun reproduces all six (log
`3213f3c0915518924b2de44d49076a1fd554dcfd9913f24ee2a116c8e0423e00`).
A read-only lookup in a concurrently rehashed debug table is not accepted as a
signal-safe fix; a debug-only non-owning frame executable view is being audited.

Per-family isolated native diagnostics now pass **13 of 15 families** after
correcting the public error type and replacing unavailable ancestry inspection
with a weakref witness for actual caller release. Two families still abort:
a newly isolated test-probe callback returns a Python result with an exception
set, and an ordinary expanded-call control installs a PEP523 hook during keyword
binding after default dispatch was selected. The latter violates a stale
post-binding debug assertion. No full native pass, source37 machine-body run,
runtime selection, or performance acceptance is inferred from these diagnostics.

Canonical checker25 builds through the normal runner, with binary SHA
`5256647034431463506587f6a122dd6186d810652933f45b9590fbcece9201dd`.
Its tests-only patch separates proposal inspection from successful-artifact
validation: the three intentionally rejected nullable-registry fixtures now
assert the actual `BlockingDiagnostic`, rather than failing inside a positive
helper. All **four focused tests**, **149 project tests**, and **two selected
resolver tests** pass. Project log is
`7f99ac1b7612fd85d49ea80906b2b4333d0afac739d4589e1cab24051adc2a81`;
the 36-test CLI gate and matching-native signed publication remain pending.
The fixed97 analysis, full-suite timings, stock/previous-strict deltas and
native-code size evidence are unchanged and still incomplete.

Workflow: a verbose rerun was correctly stopped before launch by the 10-GiB
disk reserve. Approved removal of only inactive checker incremental cache
recovered **5.94 GB of physical space**; binaries, dependencies, sources and
all evidence remain intact. Future canonical checker runs need bounded cache
retention and a descriptive reserve failure, rather than an unexplained
assertion. A transient submodule-pin precheck also needs diagnostics showing
its actual Git output; subsequent direct/recipe reads and complete replay
passed without changing the index or resetting source. Its cause is unproven.

### Native22 corrected probes and dispatch gates — 2026-08-23 16:26 PDT

The C probe now propagates an observer's error as a null result instead of
returning a value with an exception set. The newly added observer-error test
aborts before that fixture correction and passes after it. Isolated native
diagnostics pass **14 of 15 families**; only the ordinary PEP523 assertion
remains. This is not a complete native-suite pass. The corrected source-entry
family log is `d755c1b95c9c694ae4b25c78c521bcf09c45a5b5b32e45c7424042909e1571f9`.

The new ordinary controls use actual interpreters and import neither SOAC nor
the C probe. All three no-GIL diagnostic tests pass on selected optimized21;
all three fail on debug22b, with truncated fatal dumps or watchdog aborts.
The four PEP523 methods run 38 child controls: all pass on optimized21, while
six hook-installation cases fail on debug22b. Five reach the generic stale
post-binding assertion; warm expanded-call dispatch reaches `_PUSH_FRAME`'s
equivalent assertion. The other 32 controls pass, including binding errors,
hook removal, changed defaults, body-child dispatch and the next call. These
are correctness controls, not throughput measurements. Debug logs are
`0cf4b8b6547e47b5211eb3b828ad853e2a80edaf2bfe1523cd03e3ebd253a4da`
and `02e447965d81de6e52d97d90f021c7f7296b4ccd10f4e7b44921968d96e9d8ed`.

The metadata subset executes five methods with nine subcase errors, all from
late implicit FREE entries. Corrected constant-finally and nested-global
expectations pass without changing the producer. Patch0056 reconciles only
metadata with the final native maps; it does not renumber ordinary bytecode.
Patch0057 restores the independent function-layout assertion and uses the
actual frame-size probe for generators. Patch0058 adds a debug-only borrowed
executable view for unsafe diagnostic readers; runtime access and GC still use
the checked native StackRef. The view is atomically published/unpublished with
the existing code support and adds no Python owner or execution authority.

Patch0059 preserves the evaluator choice at each original pre-binding
predicate. The warm path keeps its existing guard and producer order; the
generator validates all 12 actual macro paths rather than consulting mutable
interpreter state after binding. Its nine structured tests yield three passes
and six expected missing-validation failures before the production validator,
then **nine passes** after it. This actual generator-only AFTER does not imply
a rebuilt C-runtime pass. The AFTER log is
`c8958320df7759c2725c786c1aaba9dc8dec6b008d8710e68d6309c1a6f5ee62`.

The reviewed0056–0059 patches and six corrected repository test files are now
installed. Actual canonical regeneration passes in **18.05 seconds** (log
`1085e06c256457f9a890b835385566d91febc95b475e6c8093e4526b99227758`),
and a fresh complete 5,558-file replay matches the source. New generation is
`317ca05aae4ede5156555058c2ce349530ac8dea407f0d78af8845cf837377e4`;
source-ready receipt is
`99ae0236ed9394a39a57c1578b33063e085a90905c432e8af8ee39f223f4f407`.
Fresh debug/optimized builds and both complete native/affected-CPython gates
are pending. Native21 remains selected; Rust source37 and the reviewed
ordinary closure-primary handoff have not been promoted or executed on22.

Canonical checker25's **36 CLI tests also pass**. The reverified checker-only
receipt `71fb954c2ce34c2589265be912abb973ba56fb5fa09c345e6872148195da5ccf`
binds the normal binary, source, four focused tests, 149 project tests, two
resolver tests and 36 CLI tests. Matching-native22 signed publication,
fixed97 reanalysis and all performance acceptance evidence remain pending.

One diagnostic driver stopped before launching tests because it compared a
generation string with the complete source receipt. The corrected driver
compares the explicit generation field and retains the failed preflight;
no source, runtime selection or validation result was changed by that error.

### Native22 complete debug gates — 2026-08-23 16:35 PDT

The fresh22c StackRef-debug build passes in **33.05 seconds**. The affected
CPython gate passes **52 of 52 files**, with **5,582 test cases and 69 skips**,
in **155.60 seconds**. This includes the actual generated-case validator tests
and the corrected function/frame-size expectations. The CPython log is
`42db2d2332031687924175d43e15c81cd4445dceabd59e22b3f9c597c626493f`.

The first complete repository-native run executes all 568 tests and has one
failure: an outgoing custom-call probe expects two owners even in its ordinary
control. Source inspection and a matched ordinary/adapter run establish three
actual owners: the caller operand, the temporary `tp_call` argument tuple, and
the bound method parameter. Both runs preserve identical release timing. The
fixture now asserts that actual count and still compares the complete events;
no runtime barrier or ownership rule was relaxed. The isolated parity log is
`539fe3318fc3f3bc185e602dbe84de40944758ca98a6d384f59875fb22c9a5a1`.

After this fixture-only correction, the complete native gate passes **568 of
568 tests, with no skips**, in **33.22 seconds**. Log:
`5de8c38bf5f6a932cba006fbd3758205fed1a67796ca59b25215d2465ef7360f`.
Receipt `10edd1bfb7bd55cce3dde6948627119d177b7f6054f491edc571a477f81f6d54`
records unchanged native sources and binaries and explicitly reuses the
earlier successful upstream gate; it does not claim a second execution.

The fresh optimized22c PGO/LTO build is running. Its native and CPython gates,
matching checker publication, Rust integration and full project gate remain
pending. Native21 remains selected. These native correctness results grant no
new JIT body admission and are not performance acceptance evidence.

### Native22 optimized and checker publication gates — 2026-08-23 16:48 PDT

The fresh optimized PGO/LTO build passes in **517.53 seconds**, with unchanged
source and selected-runtime receipts. Its native suite executes all 568
selected tests: **567 pass and one expected StackRef-debug-only test is
skipped**, in **13.22 seconds**. All **52 affected CPython files pass**, with
**5,582 cases and 80 skips**, in **61.27 seconds**. Logs are
`8ceb16174723d55b528b0bb0b408758daab04c90c34dcb0a346aa48dff0f9131`,
`09b7110bb22dff5cd26ffb1388a168103993be4b56da495db5e45eb4ad58de84`,
and `a7c6740cfbb00cc470c0449d0ac0c6b15ed4f37ca6eb1b113233af14667cd329`.
The six verified native gates are bound by unselected receipt
`974de078dfbf5cdcc1fd123b607ed16bfd96d45610534ad92fdcb0fcdbdcb36b`.

All **14 repository patch-workflow tests pass** on the new canonical source
(log `902497f44d0e92e68d3d0be064482dbb84cc651e7fc0121a3c9a0ae5a628f475`).
Those generator/preparation tests use the unchanged selected21 interpreter;
they are not additional22 runtime executions. An initial overlapping preflight
was refused by the native build's source lock before any tests or artifacts
were created. The successful rerun occurred after the build released its lock.
Preparation verification must be scheduled after an active native build, even
when its consumer is nominally read-only.

The actual normal checker25 executable signs a dataclass/inferred-field/function
fixture against optimized22, then repeats publication with byte-identical
deployment, manifest and shards. Both CLI invocations pass in **6.59 / 6.64
seconds**, and the source initializer's sentinel remains absent. The record
binds the real target executable and loaded library, not an alias or an old
checker receipt. Publication-ready receipt:
`56d172e1722a6bb92cef2e49caf4d1c8e6802a163ef883f3abffd836276ca6fc`.
This proves offline publication, not runtime admission or extension readiness.

New immutable Rust source38 includes tests-first constructor, guarded setter,
native retirement, ordinary FREE-primary and binder controls. Its actual22
JIT test target type-checks. The combined check finds four constructor fixture
API errors; the lowerer check finds a fifth, an invented `CellRefLoad` variant
instead of the actual `CellRef`. These are compilation failures, not behavioral
RED results. Separate tests-only corrections preserve every intended predicate.
Source review also identifies a binder-fixture error: replacing nested-function
defaults after module sealing is legitimately forbidden. Its corrected control
must use the real initializing/pre-Ready interval, not thaw the function or
manufacture execution authority. Native21 and extension47 remain selected;
Rust integration, fullgate, fixed97 reanalysis and all timings remain pending.

### Matched Rust before-cases and extension boundary — 2026-08-23 17:22 PDT

Immutable source39 passes the combined JIT/lowerer/optimizer test-target check
against the actual optimized22 library. The constructor fact regression
executes **six tests: four refusal controls pass and both positive value-fact
cases fail**. The guarded setter regression also reaches its intended failure:
Cranelift rejects an SSA value defined only in one branch and then consumed
after the join. The respective logs are
`da77a3a8f0c30db7cb6ddc7ad3e01c004d1149bdc1615ba00b867c24edbcd313`
and `340179c4fe94460cf36590d4cd0ec941a9d4720b6e2ecab269a1c8f51b792594`.
These are genuine before-cases, not rendered-IR assertions. Five independent
ordinary native FREE/binder controls also pass; log
`98a8b5b49e8a98350da77a09e319f7153ffb87c29d2415822c5f683412935db0`.

The first scope run lacks the explicit strict future in two new fixtures and
fails before testing ownership. The source40b Name fixture check subsequently
finds two actual-API errors (`Positional`, not `Arg`, and a pattern match for
the non-`PartialEq` unresolved-name enum). Separate fixture-only corrections
retain the predicates. Source40c passes the actual combined test-target check
for JIT, lowerer, optimizer, core, typed IR and CPython support in **38.24
seconds**, log
`8cf787b8b173d7d01e13fd59c698a3ad94e84084b5a4364b0dc45260fca4ab60`.
Neither earlier fixture failure is counted as a behavioral regression.

The source39 extension builds successfully, but actual import reveals
`undefined symbol: dp_jit_unpack_fixed_slow`. Promoting the raw runtime to a
normal dependency retains its native call to this C symbol; the real existing
slow helper was registered only in the JIT symbol table and lacked an unmangled
native export. The source40c correction exports that same implementation and
adds a direct raw-runtime success/error regression. Its execution remains
pending here. The retirement runs first failed because no extension was staged,
then because this genuine link error prevented import. **No retirement-body
before-result is claimed from either run.** The import-failure log is
`e21445121b0b82a6505ce5a6623beccba31b0dd6967a74fa1164f235e586a8ce`.

Environment/workflow: compilation reached the 8 GiB running reserve and was
stopped before a test ran. Approved cleanup removed only inactive incremental
caches, preserving sources, executable artifacts and evidence. A later
build-start preflight also refused the 10 GiB threshold. Already-built test
execution uses the unchanged 8 GiB running reserve without invoking Cargo.
The embedded-test staging helper also ignored an explicit `CARGO_TARGET_DIR`;
source40c derives both extension and staging paths from the same absolute
target and rejects ambiguous relative/empty values. Five focused helper tests
are included, with execution pending. Handwritten incomplete patch context
was replaced with an exact source-derived diff before application; the failed
dry run is retained. Native21/extension47 remain selected, and the full project
gate, fixed97 reanalysis and performance acceptance are still outstanding.

### Matched native entry and integrated Rust checks — 2026-08-23 17:49 PDT

Source40c's matching extension builds and imports against optimized22. The
direct raw unpack success/error regression passes, and the actual native
identity-body control passes through one C call and 512 warmed VM calls.
Failed-Begin cleanup and first-finalization allocation-failure rollback also
pass. The latter uses the real source-entry allocator seam, not a fabricated
artifact. Logs, respectively:
`98da4979127df6bf2b5d5432e1886c562b88e84702f933f060d24f69b0f5f54f`,
`dff59ffe1fb42c54cdb0ac9d103e637758299d8f1c469b8d34c94a4febc67aa9`,
`14f199907e7f72d2673d501692133a8755f882633b38a37309d82c72a419c05f`,
`1ac99ec51c4f8915c17629cdd044b0ca3591a81a10eb71dcf4f37249030556bc`.

The two retirement before-cases now reach their intended behavioral failure:
normal completion and pre-body refusal release the two actual parameters in
forward rather than reverse native-slot order. Logs:
`a4ae3135b4bc11ff9bceada00ad8bf90e0ecf603e6081e385248f1b772081cf3`,
`b913bd8f52e5ac1cbb4c00a714ac711eed2d6c68b2214366e4a0fe9e1ae8f7c3`.
Name provenance executes ten tests with seven passes and three intended
producer failures; the two ordinary FREE-entry tests reach the intended
missing-handoff refusal. These results justify the separately composed
producer, owner-handoff and retirement corrections; after-cases remain pending.

Two traceback fixtures incorrectly read `tb_lasti` as a sentinel. The approved
inserted-frame policy instead raises `NotImplementedError` for unavailable
`tb_lasti` and `tb_lineno`. Tests now require that public behavior while
retaining their frame/code identity, reference-count and release predicates.
This is a test correction, not fabricated source-location support.

The combined source41 checks exposed integration errors in imports, allocator
callback signatures, an eager-comprehension ABI initializer, explicit raw-pointer
borrowing and archive-test type paths. These failures are compilation results,
not behavioral before-cases. Source41e contains separate exact corrections and
is being checked with `--keep-going`. Review also found that suspended-factory
selection must cross-check exact parameter names/kinds with native slots;
count/ordinal checks alone are insufficient. Its negative tests and the
single-code-owner generator bridge remain separate pending work.

Workflow: a standalone `soac_cpython` test target could not locate the out-of-tree
libpython. The pending fix uses the existing `build_support` link hook, with an
actual offline Cargo-generated dependency-only lock update. Isolated Rust
snapshots now use `CARGO_INCREMENTAL=0` after repeated inactive incremental-cache
growth reached the disk reserve; source, binaries and evidence were preserved.
No selected runtime, complete-suite result or performance claim changes here.

### Matched owner cleanup and lifecycle evidence — 2026-08-23 18:38 PDT

The actual optimized22/source41e follow-up passes the ten Name-provenance
tests, all six constructor controls, the formerly invalid guarded-setter SSA
join, all six native entry/retirement regressions, the real first-finalization
allocation-failure case, eight standalone CPython-support tests and seven cache
tests. Native entry coverage includes the real C call and 512 warmed calls;
normal and pre-body parameter retirement now follow descending native-slot
order. The immutable focused audit records the isolated Rust child and parent
as one test, not two. Source41e still refuses the eight ordinary-scope and
partial-cell cases before their ownership assertions because native protection
receipts are missing; those are not behavioral passes.

The source42c composition passes test-target checking across eight crates and
the complete optimizer library suite: **269 passed, zero failed**. The four
remaining old scalarization expectations now retain their positive source,
constructor, field-plan and inlining checks while requiring all six owning
mutators to leave the actual function unchanged without a retirement proof.
A test-only assertion in the plan-owning module avoids widening production
visibility. Check and optimizer logs:
`e7bfd440fd491b37d97107c29fc8432cb9dfc56763a190ad1e414839f63a0526`,
`5d01e38ef923935f0db2b16fabe6517782bcd78057e78fc299df903b52c33601`.

The expanded source41e class gate executes **52 tests: 35 pass, 17 fail**.
Fourteen failures require the missing scope lifecycle evidence; the other three
expose obsolete entry-kind expectations and an invalid annotation-code
replacement. A real signed native-reference method has both its native strict
owner and source-entry registration, increments its native-body counter and
rejects an invalid argument. Compiled witnesses therefore accept checked-native
or genuinely registered native-reference entries, never entry-interpreter
fallback. Framework controls preserve the exact selected class source and now
compare ordinary and strict-fallback mutation, including closure and argument
arity of actual annotation code. Their corrected rerun remains pending here.

Six new ordinary native completion controls pass, including a warmed actual
`FOR_ITER_GEN` path. They establish completion-value retirement before iterator
retirement, and actual result discard/publication before target restoration;
GC is disabled during the finalizer observations. Log:
`ab198ec31a7cde454044a61bb57cbadee4ac171ccb9734cec339c7eab3a54c5f`.
Seven positive native wire5 before-tests reach the existing protection-gap
refusal; two new-schema methods instead fail the expected wire-version check.
The nine methods produce eleven failing subtests and no errors/skips, not eleven
independent methods. The two ordinary generator-construction/code-mutation
oracles also pass, preserving captured code and unused-before-first retirement;
log `8be81c7c8381b989cafc01749a4860a3414dde574e4dd432d1e2dff0c5677ca1`.
Wire5 producer/consumer implementation, strict after-cases, single-code-owner
generator transfer and no-region suspended handoff remain under development.

Workflow correction: `cargo build -p soac_jit` built an rlib, not the Python
extension. The first source42-labeled Python fixture run therefore actually
used the verified source41e extension; its nine passes and three fixture errors
are retained with an explicit artifact audit, not attributed to source42c.
The corrected build uses `soac_pyo3 --lib` and a structured Cargo `cdylib`
receipt naming the exact source manifest, output and hash. Actual42c extension
SHA is `7e52c3c3abff43fd2b25d923fd58b4fdad2bf3dfc68157e412ce53c7a1bd8f55`.
A separate real native22 virtual environment supplies test dependencies to
isolated child processes; it does not spoof interpreter identity or change the
selected native21/extension47 environment. The full project gate, fixed97
reanalysis and all required performance timings are still pending.

### Framework fallback and no-region factory ownership — 2026-08-23 18:45 PDT

The corrected framework controls pass all three actual native22/source42c
cases: ordinary Python, strict compiled entry and strict entry interpreter.
They replace and restore both method and lazy annotation-provider code while
preserving the selected framework class source. An unsealed fallback function
can retain construction provenance; its non-null native owner is not a seal or
execution grant. The controls require zero strict ID, required-boundary and
source-entry grants, together with successful ordinary mutation. Log:
`0e01dc90ff3f30a54cc3d5867cb3344eefe4fcb100c8b87b837573d59735bfae`.
The earlier assertion that every fallback owner pointer must be null was a
fixture error, not a reason to weaken enforcement.

No-region suspended factories now have a genuine ownership before-case,
independent of the missing comprehension lifecycle receipt. All **18 ordinary
cases pass**; both strict modes execute every one of those 18 observations and
then fail the intended parity assertion. Generator, coroutine and async
generator factories, with and without captured cells, retain two additional
function and code references while created, versus one of each in native
Python. Explicit function/code aliases add the same reference on both paths.
Closing and destruction recover the ordinary counts; the defect is the extra
live owner, not a finalization leak. The direct preserved-capsule edges confirm
the duplicate function and raw code ownership. The gate reports **20 pytest
nodes: 18 pass, 2 fail, zero skips**; log:
`440f978642f689098255e01548d8abfe074c4c01958af01bb719b4ec728fa5a0`.
The planned fix reuses the validated suspended source handoff and single native
code owner; it does not grant a token body or add a constructor-only refcount
exception. These after-cases and the full class-family rerun remain pending.

### Lifecycle integration and migrated tests — 2026-08-23 19:18 PDT

The immutable source43d and source43f test compositions pass test-target
checking across eight crates. Their production runtime remains source42c, not
the pending lifecycle or owner-transfer changes. Actual Python checks pass
both branch cases and all three nullable `sys.modules` sentinel cases. The
remaining migration gate executes **50 cases: 39 pass, 11 fail, no skips**;
including the two earlier branch cases gives **41 of 52 passing**. The failures
are six real offline-checker rejections, two missing comprehension-protection
receipts, and three panics on compiler-generated `__static_attributes__` stores
to captured cells. The original sources and failure logs are retained; checker
diagnostics have not been suppressed. Migration log:
`3268d5455ecabb33f2b2acae8642c642728d54e67c047bf36ba09b5cc8326b01`.

The new owned-decoder negative test reaches a distinct real defect: removing
the required suspension obligation from an actual generator recipe is still
accepted. Its before-log is
`6e9638b3266c1ad46be8ce99325214fe22516bdb1934faff4368886b9ce3104b`.
The three plain-factory lowering tests also fail, but at missing scope
attachment before their deeper negative predicates; those are not three
independent semantic negative reproductions. Their log is
`8385d943e634ec2217157264e9b9fa3cfd6b5f3350c47119e93380624f253779`.

Wire5 native lifecycle metadata is now canonical patch0060. Fresh full replay
matches all source files, and the canonical generated-case check passes with
no generated changes. Source generation is
`8ff07296728f74213321def18950ce7ba56c9f0df30440e8573b977fd9416af1`.
The unselected native23 debug build succeeds, but its **577-method** native
gate reports **51 subtest errors** at the old event phase/kind validator; no
skip or successful gate is claimed. Log:
`e4b7da03b4d2c40e937d702236f80ea1b1d94ad08eb442a633102adffd9dbe72`.
Optimized build and matched Rust execution are held for the correction.
Independent review also found that relocation-equivalent normal schedules
with explicit missing-protection gaps must remain catalogued as unsupported;
they must not be rejected as missing divergence or admitted for projection.
The separate focused correction preserves all protection refusals.

Three actual native22 ordinary retirement controls pass: suspended close,
cyclic GC, and the separate refusal of suspended `frame.clear()`. Both cleanup
paths release the generator and payloads, preserve the handled exception and
return the code count to baseline. Close drops `unused` before `first`; cyclic
GC observes the reverse order. Reentrant close succeeds in each natural
retirement boundary. Direct escaped-capsule clearing is a different boundary,
not an ordinary `frame.clear()` equivalent; strict parity and allocation-fault
after-tests remain pending.

Workflow: the mounted host volume approached its reserve while the guest had
ample capacity. New native builds and the native23 Rust target use guest-local
storage; source, selected native21/extension47 and all failed evidence remain
intact. Test composition also exposed two test-only Rust API mistakes and
zero-fuzz append-context conflicts; corrected fixtures preserve existing
observers and assertions rather than weakening production types or applying
fuzzy patches. The full gate and fixed97 performance measurements remain open.

The native23b event-domain correction removes all 51 errors. The next full
debug run still fails: **577 methods, six failing subcases and one stale
11-field tuple unpack**, zero skips; log
`f75e8fca324a2fa2c002853a0419f334e99c6a8c60899cdb44fb70452128e909`.
The tests-only correction executes 14 focused methods with six failing
subcases in three methods, no errors/skips. Both new controls pass: a reachable
fallthrough retains its three exact contexts, while a wholly eliminated
original retains its explicit refusal. Log:
`67bcdd595424428345d3964dc16d7fa7cc0af08af86f39fa9bba98da718d84f1`.
Actual byte/handler inspection finds three complete surviving allocation
copies and one unreachable codegen fallthrough copy. Pending patch0062 records
the real CFG reachability decision before instruction removal and validates
its original allocation identity; missing final metadata alone never counts
as proof of elimination.

An unselected nondebug development build permits iterative JIT checks without
changing the final optimized-build requirement. The wire5 consumer and its
tests pass checking across the eight crate test targets; log
`5437ec86bf0f5a853988f48a0f2f123cb0786d22a37b3b32283333a2b50e9dfc`.
The normal-equivalence regression then executes: the malformed-input control
passes, and the coherent-normal/explicit-protection-gap control genuinely
fails at the missing-divergence rejection. Log:
`ee7ba4809aaa77f5c720b7bbd1787fa9a645b882dd8afc6c1b9d7e3f1e7a411b`.
The five parameter-join tests all stop earlier, constructing their shared
positive fixture: a native hidden local and same-spelled FREE slot select the
same raw owner. Their negative edits are not reached; log
`06d76c57d46bab7fa9258d074fd51236ff1b627839139cc279e67d24833ed241`.
This requires a distinct native-slot mapping repair, not weaker uniqueness or
five claimed negative-boundary reproductions.

Two preliminary check attempts are retained as workflow failures, not code
regressions: a misspelled package name, then the JIT build's deliberate
`Py_REF_DEBUG` refusal. The successful retry uses the existing nondebug
development recipe and the previous verified eight-package command.
`AGENTS.md` and `README.md` now explicitly distinguish this Rust/JIT iteration
mode from native-only StackRef-debug diagnostics. No full-gate or performance
result is inferred from these focused development runs.

### Native23c lifecycle correction — 2026-08-23 19:57 PDT

Canonical patch0062 records positive CFG-unreachable allocation observations,
not merely absent final events. The unchanged surviving-copy validation and
the reachable/wholly-eliminated controls now pass together. Fresh patch replay
and generated-case verification are equal; source generation:
`e9b9b38caa814c4401e426525b1d5f873759f97c98a19179b76842a39fbcb43b`.
The actual unselected StackRef-debug interpreter passes **579/579 native
methods, zero skips**, then **52/52 CPython files, 5,582 cases, 69 expected
skips**. Native log:
`ef5b7779fd8d18b6fc2ea343f0eeb263b35ee2f41b59a0245c3c6201945aff1c`;
CPython log:
`2b11142aa79fbd108926a4e7116dc50edc479f86a9b69654d05e8b7cd7e1746f`.
The optimized build and its gates are still pending at this checkpoint.

Matched Rust44c contains the normal-equivalence correction after its genuine
before failure, plus unrepaired class-tail and suspended-retirement controls.
Its source-ready hash is
`f38ebd0fdd01b5e549df17c4957bad0be4ad2b332ecaa0926c08895182dbfbdd`.
The added retirement tests cover close/GC finalizer reentry, first Borrow-frame
allocation failure and direct escaped-capsule clear; they are not yet executed.
The code-only owner bridge, plain suspended-factory correction, parameter join,
and compiler-created class-tail operation are not in this snapshot.

The native-slot collision needs the actual native region/access and resolved
lexical binding: function comprehension targets can be plain LOCAL without
`CO_FAST_HIDDEN`, so the flag alone is insufficient. The exact distinct-owner
validator remains intact. The class-tail ownership review requires borrowing
the authentic class-code constant for Name/Global stores, one stored-reference
promotion for Cell, and the existing code lifetime through callbacks. A helper
code object, extra temporary Python owner, or source-name exemption is not an
acceptable substitute.

Workflow: two tests-only patch contexts overlapped later fixture changes; exact
family/method reanchoring preserves all prior finalization and cleanup code,
with a separate context proof and zero-fuzz replay. An attempted simultaneous
development build was rejected by the shared CPython source lock before
creating a build artifact. Native modes/preflights must run serially even with
separate output directories; this is now explicit in README and AGENTS.
These are focused native results, not a full SOAC gate or performance evidence.

### Storage failure and recovery — 2026-08-23 20:14 PDT

The optimized23c build completed successfully before the storage incident
(517.96 seconds; log
`98d22b64c1fc4c2f8b515cf665cb39640df2d0e42791210bb63645519b9f7375`).
Its later native/CPython tests had not started. An approved attempt to move the
old generated native22 Cargo target copied into the guest disk while checking
only guest capacity. That was insufficient: the sparse image uses the same
host volume, which had less free space than the duplicate. The copy failed with
EIO before renaming/removing the original or installing a symlink, and guest
commands stopped launching. All project work was paused.

Recovery removed only 335 approved rebuildable `.rlib` cache files; original
sources, logs, test executables and the measured extension were retained.
VirtioFS held deleted files open, so space was reclaimed only after the VM was
stopped/restarted. APFS reported no snapshots. The failed guest copy was then
removed and unused guest blocks trimmed. The original cache path was never
switched. No source mount, native selection or venv selection changed.

The recovery verifies **5,707 retained files** against their recorded hashes,
including original native builds, debug/optimized23c, checker25, the old actual
extension, native source inputs and Rust44e. The native generation is unchanged;
the shared CPython source is still on writable `lima-63b0316c44311daf` VirtioFS.
A guest write/fsync/read probe passes. Retained-file receipt:
`3cc0aee54150d245a96499708bf6db865efa930130e54a49858f185cc95c8484`,
under `work/native23-disk-recovery/`. Subsequent optimized gates remain required.

Workflow correction: check shared-host capacity as well as guest capacity,
budget the full duplicate before cross-filesystem moves, and verify reclaimed
space rather than assuming unlink released it. README/AGENTS now state these
requirements. The failed transfer is not performance evidence. A separate
snapshot preparation also rejected a wrong manually supplied patch digest;
later composition reads digests from the frozen manifest before copying.

### Native23c complete gates and Rust44g discriminators — 2026-08-23 20:32 PDT

After recovery, optimized23c completes the actual native suite: **578 pass,
one expected StackRef-debug-only skip**, 579 collected/executed. Its selected
CPython suite completes **52 files / 5,582 cases**, with 80 skips. Debug23c had
already completed **579 native passes, no skips**, and the same 52 CPython files
and 5,582 cases with 69 skips. Both native configurations are now gated; neither
is selected and no matching extension is declared ready. The combined receipt
is `work/native23-lifecycle-v3/gated-unselected.json`, SHA-256
`92ee983f9934cd9a6ad63441e6145a20b5aff79fbb502d90685c14684a5f77f4`.
Optimized native/CPython logs respectively hash to
`6693275f34797f567064e972533c711b1b23ad75c2881d86afe90a2a8911e858`
and `cf840f650b3df3de688716aa33457137bd4d348a374751267911d49484e87355`.

The Rust44e and 44f checks exposed test-only compilation errors: one new fixture
used a nonexistent IR type path, then two PyO3 closures needed explicit result
types. Separate minimal corrections preserve their source programs, assertions
and frozen upstream patches. Those failed checks executed no test bodies.
Rust44g passes test-target compilation across all eight crates in 32.10 seconds
(log `3d2b378bc6eee36cc320cd37bd1230435258664e0b02b013a855439fa4187d70`).
The execution wrapper now records actual harness summaries separately from
compilation and the native interpreter probe.

The normal-equivalence correction now has actual after-evidence: **two pass**,
including malformed lifecycle rejection, against the earlier one-pass/one-fail
before-case. Log:
`a3cd075516e34f8e49855876a85942982dead6e82002e095959d25f2ca18fac8`.
The hidden-current tests reach three genuine before-failures: ordinary and
suspended LOCAL/FREE storage collide, and a hidden comprehension CELL replaces
the lexical Global role. Log:
`64e0803dd688728f82eb8791b620cdd7d32e6c44cb516f17f7402dadcdf2e0d1`.
The three original class-tail fixtures also reach their intended before-panic:
compiler-generated captured-cell stores have no original source Name access
receipt. Log:
`44adc7916bb8e302614f25c8fb6482767ee23a228f0c94b7aceb1c3eabe402a1`.
No negative mutation reached only after successful lowering is counted as
executed when fixture construction fails first. Matching after-cases, the
combined runtime, full gate, and all performance acceptance remain open.

### Actual carrier and checker boundaries — 2026-08-23 20:54 PDT

The hidden-current correction separates ordinary and suspended LOCAL/FREE
storage: both focused tests pass. Its third test now reaches a later, genuine
suspended-global defect. The original global `outer` read at source 148..153 is
selected as `Cell(CapturedSource(0))`, with an invented inherited freevar; the
ordinary variant passes. Rust44k's typed diagnostic preserves the original
Global expectation and actual slot identities. Log:
`e5a15067882d074e0067142d6812b27e56b2b05c72f77361f2695d36b63f27ad`.
The correction is not yet complete.

Mixed-parameter tests can now construct their genuine source fixture. The
positive test's physical-name expectation was wrong: preserved carriers use
the existing public scope's `cell_storage_name` mapping, even for non-CELL
parameters. The separate corrected test preserves all logical/native identity
checks. Actual Rust44k results are **one positive pass / five negative failures**:
changed argument kinds, swapped parameter names, a missing public binder,
relabelled logical identity, and identically renamed public/resume physical
storage are all still accepted before the proposed validator. Log:
`70b5f9833acd007dee11db24969f4bc5ea89541cba3c32a19ca56a0cb483379f`.
The proposed validator also needed that exact producer mapping, not a literal
storage-name equality. A test-only whole-scope `PartialEq` assertion first
failed compilation and was replaced by the relevant mapping comparison; no
production equality implementation was added.

The managed-code controls execute **two passing ordinary controls**, but all
nine strict methods stop before their ownership observations because their
test catalog omitted the lambda present in the unchanged original source.
Log `40d08bc6a03185a25d4a3e55425119f0db09d0c981d2626acf2b48d4c52adf81`
is therefore an authentication-boundary failure, not nine ownership failures.
The new fixture adds only that exact Lambda identity and unchecked signature;
native source matching and all original lifetime assertions remain mandatory.

The isolated R13 checker tests run against canonical25, with its selected
binary/source/pin unchanged. BEFORE: **three pass / four fail**. Parent
production AFTER: **six pass / one fail**. Before/after logs hash to
`e3a8d6b2ccb511740c376788681e2f40dd77af413ae9fec6502eae8ab87ad178`
and `e295cf9b36a6a923ee16fa5d51b6a481c7d581ce45f30e3c03aeeb5d805d188d`.
The remaining fixture incorrectly expected inferred FunctionLiteral returns
from an unannotated factory; the configured analyzer returns Unknown. Its
separate correction keeps the original source, tests Unknown/no executable
authority, and adds a repeated-definition loop that genuinely shares the
FunctionLiteral type across different runtime objects. The corrected test and
class-valued/returned-member follow-up gates remain pending. The first result
reader rejected Cargo's relative executable path after seven test bodies ran;
its preserved log and a corrected-reader replay give the same before result.

The four new native compiler-tail methods execute on unchanged23c before
wire6: **16 intended missing-receipt failure subcases**, no errors or skips.
Log `34b59c0c32f1113b0f7db774a8f8262db22b1e4b58f9a454007e9498e30f92a2`.
The native wire6 proposal has been reviewed but is unbuilt; the actual class
namespace/global/cell tuple/key borrowing controls and matching Rust consumer
are still required. No complete compatibility or performance claim follows.

### Matched extension and independent ownership baseline — 2026-08-23 21:11 PDT

After the lambda catalog correction, the nine strict ownership tests reached a
different setup failure: `_soac_ext` was missing. ROOT initially read the live
checkout's helper and incorrectly attributed this to a hardcoded target path.
The actual44k/44l snapshots already honor an absolute `CARGO_TARGET_DIR`; the
immediate missing prerequisite was their built `soac_pyo3` cdylib.
That run is not ownership evidence (log
`023e077bcd87dd91b3caf45715f9d04b863e3a2b1c3e47b752bf5bfee034e7d9`).
An actual `cargo build -p soac_pyo3` against gated optimized23c now succeeds;
the resulting cdylib hashes to
`e9d1a6ef3ec39f2eca46b1cd1f348aedc0273893c42702259d75f523e36fbb96`.
A redundant copy was made only into the ignored44k snapshot, with a real
isolated import checking that copy and the actual interpreter. The Rust test
helper instead uses the absolute Cargo target's `debug/test-ext` symlink;
ROOT subsequently verifies that symlink and its artifact have the same digest.
The original copy-import receipt remains at
`work/native23-rust-matched44k/actual-extension-staged.json`; it is not an
observation of the Rust process's import path. The next preflight observes the
actual target's mapped extension. Live venv, native selection, source and
checker remain unchanged.

The first actual ownership assertion then fails: the native generator does not
take the sole code owner, and the retained code count increases. The shared Rust
test lock poisons after that panic, so its ten subsequent failures cannot be
counted independently. Fresh-process, exact-test reruns now give **two ordinary
controls passing / nine strict methods failing**, with no poison cascades.
The actual test executable supplies the test inventory; prefix-matching source
functions had initially included a non-test helper and was rejected before any
test ran. The final receipt is
`work/native23-rust-matched44k/managed-code-independent-before.json`, SHA-256
`5d17c98f8d5246d6f5c17a7b97acbf380e31fb86691ee4e0256b4c163679b57a`.
The failures reach raw-slot/code-owner observations, real allocation-error
observers and reentrant sink/retirement checks. A method that stops in its
compiled iteration does not establish its later entry-interpreter subcase.
The reviewed ownership bridge is still unapplied, not an after-result.

Checker parent R13 plus its explicit factory-fixture correction now passes
**7/7** (log `314667c091e9e6e528347b749628cc03b27cd4d4c90461fff80b3d8c899e4d77`).
Class/callable provenance controls give **8 pass / 9 fail before** and
**15 pass / 2 fail after**; logs respectively hash to
`a5ce1b5b4bdc2111220b67150ed3728997f50046d1325fb1253ab8ceb47a3342`
and `52068be75b5cfabbecf063912d9cef592f39fe69dc7ed7afe8568f77a6438da2`.
One remaining failure requires preserving an already signature-only Callable
constraint without restoring receiver/target/layout authority. The other is
an invalid fixture expectation: `Type::nominal_class` does not manufacture an
instance declaring-class receipt for a ClassLiteral. Its exact rebound target
assertions already pass; a separate direct-class control will verify that
existing boundary while preserving the original source. Both corrections are
frozen and their actual gates remain pending. No checker is promoted.

The ordinary class-metadata probe executes all fifteen original programs, but
its first version finds no stores because final CFG processing propagates
source locations onto generated instructions. That log is retained as ordinary
observations, not compiler-tail or constant-identity proof. Its diagnostic-only
correction enumerates all exact-name stores and explicitly leaves their origin
unclassified; only native compiler receipts may authorize tail operations.

Workflow improvement: preflight an actual matching cdylib import before embedded
runtime tests, inspect the helper compiled into that exact snapshot, resolve
staging from the actual Cargo artifact/profile, and run intentional shared-state
panic baselines in independent processes.
These gates are still isolated correctness work, not full-suite or performance
acceptance.
