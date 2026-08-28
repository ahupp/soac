# Single-file strict integration scenarios

A file under `tests/strict_scenarios/` defines shared module setup followed by
any number of independent test blocks. The tree is grouped by theme, with
further subdirectories where useful (for example `modules/bindings/` and
`imports/regressions/`). `tests/test_strict_scenarios.py` discovers **every
`.py` file recursively**, using its relative path and backend as the test ID.
These are source fixtures, not Python modules imported by pytest.

By default each file runs in compiled SOAC, entry-interpreted SOAC, and strict
CPython. Each file/backend is analyzed once with the actual vendored checker;
every block then gets a fresh authenticated process and fresh module instances.
Setup code is shared in the file, but mutable runtime state is never shared
between blocks. No native facts are manually installed.

## Format

```python
# module:mod1
# soac: module(checked_attr=true)

class A:
    foo: int = 0

# ok

a = A()
a.foo = 1
assert a.foo == 1

# raise:TypeError

a = A()
a.foo = "str"

# ok

assert A().foo == 0  # The earlier blocks cannot change this case's setup.
```

- `# module:name` starts a Python source module. All module sections precede
  test blocks. Multiple modules can import each other using their declared
  names. The first module supplies the bare names used by validation blocks.
- An optional `# modes:soac,entry` or `# modes:cpython` header, before the first
  module, limits backend enrollment. It may list any nonempty subset of the
  three distinct names. Use it for a genuinely backend-specific subject, such
  as an ordinary CPython frame control; do not replace assertions with empty
  backend conditionals. Invalid, duplicate or misplaced mode declarations fail
  collection. This is test enrollment, not a source strictness rule.
- Module sections contain the production `# soac:` rules. The adapter does
  not insert a future import, comment rule or config file. Unselected sections
  stay ordinary; package/module/class settings compose exactly as they do in
  production. The checker receives the original section source, with no
  manufactured facts or native authority.
- `# ok` requires the complete following block to finish without an exception.
- `# raise:TypeError` requires **only the final top-level Python statement**
  to raise that exception class or a subclass. Every preceding statement runs
  outside the expectation. The expected name can be a builtin, a class defined
  in the block's setup/primary module, or a qualified `module.Exception` name.
  Exception-name resolution is also outside the expectation.
- Directives are standalone column-zero comments. Strings, indented comments,
  comments inside bracketed expressions and trailing comments are not markers.
  Prose such as `# module opt-in` or `# raise only on writes` is not a directive.
  Missing/empty blocks, duplicate modules and malformed directives are errors.

In the `raise:TypeError` block above, a `TypeError` from `A()` is a **failure**. Only the
assignment can satisfy the expectation. Semicolon-separated statements are
distinct statements too. A compound statement such as `with` or `if` counts
as one top-level statement; prefer a final simple operation when that is the
precise boundary being checked. Syntax errors are setup failures, never matched
runtime exceptions.

There is no validator-function convention inside a block: its statements
execute directly. Defining a function does not implicitly call it.

Run the complete tree through the dispatcher:

```bash
just pytest-fast --require-batch-runner tests/test_strict_scenarios.py
```

`just test-all` includes this dispatcher through its normal `tests/` selection.
The parallel runner isolates each file/backend in its own batch. Its outer
deadline is the configured batch base (300 seconds by default), plus 120
seconds for each block after the first, using the actual parsed block count.
Every runtime process still has its unchanged 120-second deadline; checker
deadlines are unchanged. A failed or timed-out block is recorded while later
independent blocks continue. The worker retains its bounded aggregate deadline
and process-group cleanup. A single file/backend can be replayed with
a selector such as
`tests/test_strict_scenarios.py::test_strict_scenario[fields/basic_assignment-soac]`.

For packages, declare the parent and each selected child, for example
`# module:package` and `# module:package.child`. The parent becomes
`package/__init__.py`; name the package, not `package.__init__`. No undeclared
parent modules are synthesized.
A lone section with a package directive is also materialized as `__init__.py`.

## Mixed strict and ordinary items

Strictness is independent of the scenario delimiters. This example keeps the
package initializer and caller ordinary while selecting fields in its models:

```python
# module:package
# soac: package(checked_attr=true)
# soac: module(checked_attr=false)

# module:package.models
class Checked:
    value: int = 0

# soac: class(checked_attr=false)
class Dynamic:
    value: int = 0

# module:caller
from package.models import Checked, Dynamic

def write(item, value):
    item.value = value

# ok
import caller
checked = caller.Checked()
caller.write(checked, 7)
dynamic = caller.Dynamic()
caller.write(dynamic, "ordinary")
assert checked.value == 7 and dynamic.value == "ordinary"

# raise:TypeError
import caller
checked = caller.Checked()
caller.write(checked, "wrong")
```

Both flags default false. `strict_assign` controls module binding freezing;
`checked_attr` controls eligible class/field contracts. A package rule flows
to descendants, a module rule changes one file, and a class rule changes one
exact declaration. Explicit false never revokes installed ancestor/storage
checks. See [the production rules](STRICT_MODULES.md#source-policy-rules).
An ordinary module may be first, and an entirely ordinary scenario is valid.

## Isolation and authentication

The adapter analyzes and publishes the whole file's module set **once per
file/mode run**. It then starts a fresh authenticated interpreter for **each
block**, imports all declared modules in declaration order, and checks native
module witnesses before running validation. Selected modules must have actual
authenticated readiness and their selected binding-seal state. Ordinary
modules/functions must have no strict ownership or JIT metadata. Mutable
objects, module caches, builtins changes and test
locals from one block do not carry into the next block.

An import written inside a validation block does **not** defer initialization:
all declared modules have already been imported. If a case must change ordinary
support state before a selected module captures it, put that deterministic
mutation in an ordinary helper module declared between the support and model
sections. Keep the original support/model source bytes and policies unchanged.
Use a separate scenario file when other cases need the unmodified initial state;
do not change expected defaults or move setup into the exception expectation.
More involved interleaving that this ordering cannot preserve stays in a
specialized harness.

Validation is ordinary Python, outside the analyzed/sealed modules. It receives
a copy of the first module's namespace and an explicit `module` reference.
Consequently, `answer = 2` changes a validation local, whereas
`module.answer = 2` changes the actual module subject to its selected policy.
Classes and objects referenced from that copy are the real runtime objects.

Surviving plain, undecorated synchronous module functions receive automatic
actual-entry witnesses, including functions retained through aliases. Source
coordinates select these subjects; the actual native owner still proves their
authority. Generator factories, decorated functions and methods are not guessed
to be synchronous entries; cases needing their specific execution-path evidence
must retain appropriate explicit witness assertions in an `ok` block.
The existing `__dp_integration_mode__`, `__dp_integration_soac__` and
`__dp_integration_entry__` validation flags are available.
`__dp_integration_strict__` reports whether the first module was admitted, not
whether every module is strict. Witnesses run again after validation; mutable
module/function names may legitimately be rebound or removed, so the runner
does not reimpose binding freezing by looking them up afterward. Module identity
and installed ownership remain checked.

Parsing, blocking offline-checker errors, startup authentication, module imports, native
witnesses and block-prefix exceptions cannot satisfy `raise`. A completion
receipt rejects a process that exits successfully without finishing validation.
Block failures are retained and later blocks still run in fresh processes;
analysis/publication failure stops the scenario before any blocks run.
Ordinary-module diagnostics are informational for this contract publisher;
unsuppressed errors in selected modules still block publication. A class opt-out
inside a selected module is not a blanket static-diagnostic suppression.
The exception wrapper preserves compiler-generated annotation setup for the
whole validation block while catching only its final statement.

Supported C APIs can be exercised with ordinary Python/ctypes inside a block.
Runtime value enforcement remains field-only: annotations do not introduce
argument or return checks.

## Running

From the Lima guest checkout:

```sh
just pytest-fast --require-batch-runner tests/test_strict_scenarios.py
just pytest-fast --require-batch-runner 'tests/test_strict_scenarios.py::test_strict_scenario[fields/basic_assignment-soac]'
```

Use the documented Lima launcher from the host. Logs, source projects, signed
publications and per-block subprocess output are retained by the existing
`StrictProject` helper.

## Actual migration — 2026-08-28 (PDT)

The themed tree contains **259 files with 455 independent blocks**. Its
explicit mode enrollment selects **508 file/backend tests**, requiring **928
isolated block executions**. Shared fixtures stay together; variants needing
different module bodies or pre-import setup remain separate files. These are
source/enrollment counts, not a claim that every enrolled execution has passed.

**216 authored regular tests were migrated and removed**, not merely inspected
or wrapped. This includes 207 of the earlier 250 strict-source cases, seven
ordinary controls and two native-API cases expressible with inline ctypes.
The 18 files that existed before this bulk migration were also moved into
the themed tree without changing their bytes. Eleven tests of the retired
import/entry-specific fixture schedulers and annotation adapter were removed
with those unused adapters;
recursive collection and independent execution are tested at the replacement
runner boundary.

| Theme | Files | Independent blocks | File/backend tests |
| --- | ---: | ---: | ---: |
| Annotations | 13 | 23 | 23 |
| Classes | 25 | 44 | 44 |
| Dataclasses | 28 | 53 | 50 |
| Execution | 38 | 46 | 80 |
| Fields | 9 | 25 | 24 |
| Frameworks | 3 | 3 | 9 |
| Functions | 46 | 87 | 73 |
| Generators | 10 | 61 | 19 |
| Imports | 36 | 36 | 72 |
| Methods | 17 | 22 | 36 |
| Modules | 23 | 26 | 46 |
| Packages | 2 | 3 | 5 |
| Policy composition | 9 | 26 | 27 |
| **Total** | **259** | **455** | **508** |

**108 authored tests remain in the regular strict files**: 43 strict-source
tests with retained specialized harnesses, plus 65 other-layer tests (27
ordinary controls, eight native-ABI probes, nine external-C-fixture consumers,
and 21 preparation/worker-tooling tests). The 513 native unittest methods are
still a separate layer. These counts do not include the scenario dispatcher
or its own unit/integration regressions.

The 43 retained source tests have these primary requirements, counted once
even when a test covers several phases:

| Retained harness | Authored tests |
| --- | ---: |
| Shared profile/replay and structured counter/plan evidence | 20 |
| Expected initializer failures and terminal cleanup | 6 |
| Checker rejection and absence of publication | 2 |
| Loader construction/execution phase observations | 2 |
| Post-publication source/artifact mutation | 2 |
| Published signature-shard inspection | 1 |
| Alternate venv, path selection or background scheduling | 4 |
| Actual module-wrapper retirement | 2 |
| Interleaved imports, validation and worker completion | 3 |
| Native namespace ownership/refcount control | 1 |
| **Total** | **43** |

Retained does not mean untested or impossible to express in a future adapter.
In particular, the three retained interleaved-import tests combine imports
with intermediate validation or worker completion. Eager setup alone cannot
preserve those phases; deterministic pre-import mutation by itself is simpler
and can use an ordinary helper section. The earlier
inspection overestimated those three candidates. Conversely, explicit backend
enrollment enabled eleven cases previously in its extension/review bucket.
Inline supported C-API operations are not by themselves a missing capability.

The per-original-test mappings, original source snapshots, byte hashes and
removal audits are retained in `work/strict-scenario-migration/`. Scenario
comments name their original subjects; no scenario invokes a deleted pytest
test. The combined audit verifies module-section bytes, every original-test
removal and all file/backend enrollments. Those static checks are distinct
from actual runtime execution evidence.

### Migration execution evidence — 2026-08-29 (PDT)

The pre-repair focused runner cohort passed **85 tests in 31 batches**, including
actual recursive pytest collection, shared-setup isolation, final-statement-only
exceptions and ordinary source-origin checks. It used the verified optimized
runtime with unchanged input/runtime postchecks. This validates that checkpoint,
not the complete migrated tree or subsequent import-order/timeout repairs.

| Evidence | Status |
| --- | --- |
| Original single `just test-all` | 1,974 Rust tests passed; Python finished with 1,286 passing and 12 failing batches, with no timeouts. The original scenario tree completed 499 passing and seven failing file/backend runs. |
| Targeted repair replay | All 104 tests in 35 batches passed: 20 retained assignment-cleanup cases, ten changed/new scenario runs, and 74 runner regressions. Input hashes were unchanged and the optimized runtime postcheck passed. |
| Final scenario execution accounting | All 508 final file/backend runs and 928 isolated case executions have unique verified receipts; zero missing, duplicate or inconsistent entries. |

The failures exposed migration defects: string-based fixture dependencies had
been pruned, dataclass setup imports/mutations moved past their required import
boundaries, an extraction ignored a nested `return` and emitted an impossible
native-only parameter branch, and copied module globals shadowed a validator's
builtin `staticmethod`. Repairs restored the original subjects and assertions;
the impossible duplicate alone was removed. No new xfails masked these defects.
The replay also tested aggregate scenario deadlines and continuation after an
individual case timeout without increasing the per-process deadline.

The original supervisor's final JSON receipt was lost after a tool-session
interruption. Its final input map and runtime postcheck are unavailable. A
separate point-in-time recovery compared the immutable pre-gate tree with the
current checkout: all original scenario and runner bytes matched; the only
seven changed paths belonged to the separately developed test-case browser.
Committed native/checker source verification and a fresh **current-only**
optimized runtime preflight passed. This does **not** establish a continuously
frozen original gate or reconstruct its missing postcheck.

The final accounting preserves that limitation and the seven original failed
scenario rows. It combines original driver/completion evidence with the clean
targeted replay, rather than claiming a fresh green full gate. The replay used
a fresh tracked-path inventory, durable startup hashes, guest-side supervisor
output, and an atomic final receipt. No second full gate was run.

Retained evidence under `work/strict-scenario-migration/` includes
`execution-receipts-original-final-v1.json`,
`original-gate-recovery-current-only-v2.json`,
`execution-cohort-repair-final-v1.json`, and `execution-union-final-v1.json`.
The replay log and postcheck receipt are
`work/logs/comment-policy-scenario-migration-repair-v1.{log,json}`. Original
logs, source snapshots and failed reports remain unchanged. Collection alone
is not execution evidence: the accounting checks exact case drivers,
completion receipts, publication hashes, and native pre/post witnesses.
The older comment-policy results below remain historical. This work made no
optimization or benchmark claims, and nothing was pushed.

## Historical inspection of the 18-file starting tree — 2026-08-28 (PDT)

This inspection preceded the bulk migration above. At that point the scenario
tree held 18 files, and only two authored regular tests had been removed through
scenario migration. The counts and old filenames below are preserved historical
evidence, **not the current remaining backlog**. Use the actual migration section
for the 216 removals and 108 retained tests.

A guest AST inventory rechecked at **00:31 PDT on 2026-08-28** counted
**324 authored tests**
in the 22 regular `test_strict_*.py` files: 320 functions and four unittest
methods. This was after two original authored checked-field tests were migrated
into three scenario files and after field-write/policy reconciliation. It counted
neither backend invocations nor case-parameter expansion. One authored function
can cover several source variants and therefore require several scenario files.

Of these, **250 were non-native strict source-integration cases**.
**199 were classified as fitting the then-current format**: 86 straightforward
source/validator extractions and 113 with expressible ordinary helpers,
source-policy choices or backend-witness bookkeeping. **51 were marked for
additional runner capabilities or a separately reviewed split.** These were
inspection classifications, not completed conversions or passing-test evidence;
later implementation and review superseded them.

The other 74 regular-file tests were counted separately: **34 ordinary-only
controls, 10 native-ABI probes, nine external-C-fixture consumers and 21
checker/benchmark preparation or worker-tooling tests**. Six of the ten ABI
probes and seven of the nine C-fixture consumers used actual checker
publication; their additional native subject/setup explained the separate
classification, not a bypass of analysis. The two remaining C-fixture consumers
were ordinary controls. Ordinary-only scenarios are allowed, but moving a
control does not add a strict source-integration case.

In this historical table, **Ready** and **Ordinary/policy** were considered
expressible at the time. The latter recorded ordinary setup, field-policy or
witness work, not a proposed new directive. **Other** identified separate test
layers: **O** ordinary control, **A** native ABI, **C** external C fixture,
**T** preparation/worker tooling.

| File | Authored | Ready | Ordinary/policy | Needs capability | Other |
| --- | ---: | ---: | ---: | ---: | --- |
| `test_strict_annotation_replay.py` | 22 | 13 | 7 | 0 | 2 O |
| `test_strict_call_context.py` | 8 | 6 | 0 | 1 | 1 O |
| `test_strict_cell_errors.py` | 1 | 1 | 0 | 0 | — |
| `test_strict_cell_regions.py` | 1 | 0 | 0 | 1 | — |
| `test_strict_checked_calls.py` | 3 | 0 | 1 | 2 | — |
| `test_strict_checked_fields.py` | 18 | 4 | 10 | 3 | 1 O |
| `test_strict_class_runtime.py` | 32 | 10 | 10 | 7 | 5 O |
| `test_strict_dataclass_adapters.py` | 27 | 4 | 19 | 3 | 1 C |
| `test_strict_dataclass_decline.py` | 2 | 0 | 2 | 0 | — |
| `test_strict_dataclass_nominal_bindings.py` | 11 | 0 | 11 | 0 | — |
| `test_strict_descriptor_runtime.py` | 6 | 1 | 4 | 0 | 1 A |
| `test_strict_dict_policy_transition.py` | 4 | 0 | 0 | 0 | 4 A |
| `test_strict_entry_runtime.py` | 16 | 7 | 0 | 1 | 4 O, 4 C |
| `test_strict_framework_fallback.py` | 2 | 0 | 2 | 0 | — |
| `test_strict_function_boundaries.py` | 68 | 23 | 24 | 9 | 4 O, 5 A, 3 C |
| `test_strict_generator_protocols.py` | 22 | 11 | 0 | 1 | 10 O |
| `test_strict_import_admission.py` | 26 | 2 | 4 | 13 | 6 O, 1 C |
| `test_strict_membership_order.py` | 1 | 1 | 0 | 0 | — |
| `test_strict_method_dispatch.py` | 6 | 1 | 0 | 5 | — |
| `test_strict_module_preconditions.py` | 7 | 0 | 2 | 4 | 1 O |
| `test_strict_nominal_methods.py` | 20 | 2 | 17 | 1 | — |
| `test_strict_pyperformance_sources.py` | 21 | 0 | 0 | 0 | 21 T |
| **Total** | **324** | **86** | **113** | **51** | **74** |

### Requirements recorded by that inspection

The 51 reviewed source cases had these requirements; mixed cases appeared in
more than one count. Some were subsequently migrated with backend enrollment
or reviewed setup splits. This list does not replace the current 43 retained
source-harness requirements above:

- **20** retained profile/apply/verify state, counters or optimization-plan
  artifacts. Plain unprofiled execution would lose the assertion. This audit
  does not resume optimization or benchmark work.
- **11** had a genuinely backend-specific subject: exact native source-frame,
  closure/refcount/implicit-cleanup observations, or an escaped SOAC private
  execution handle. These needed explicit backend enrollment or a reviewed
  semantic split, not an empty guarded block. A native entry witness alone is
  not a blocker.
- **Seven** intentionally failed a selected initializer, including one of the
  profile cases; **two** expected checker rejection. Imports, analysis and
  authentication must still succeed outside a runtime `raise` expectation.
- **Two** mutated published source/artifacts before admission; **one** inspected
  emitted signature facts; **three** observed loader/module states before body
  execution. These required explicit publication/admission-phase access.
- **Four** required another interpreter environment, source-tree/path-selection
  layout, or background execution with its logs and timeout controls.
- **Two** required lifetime-neutral module validation. The runner kept
  module wrappers alive in its precheck map and `_execute_block` argument, then
  imported them for postchecks. Deleting a copied validator global cannot prove
  that the actual wrapper was collected.

The nine external-C-fixture consumers additionally required their real compiled
watcher/iterator fixtures. Inline ctypes calls to already-supported C APIs do
not themselves need a new format capability. Manual native capability/policy
construction and private implementation-payload replacement remain distinct
ABI tests, even when they start from authenticated source.

Ordinary helper sections can perform deterministic setup before selected
imports. Framework comparisons must keep their isolated ordinary process,
using the same interpreter and dependencies; an inline ordinary subprocess is
valid validator code, but importing both framework registries into one process
is not an equivalent control. Likewise, ordinary generated-dataclass and
stdlib dataclass-helper frames keep their ordinary observer behavior. They are
not excluded SOAC source-frame observations.

The inspection separately counted **513 authored methods** in ten native files:

| Native file (`test_strict_` prefix) | Methods |
| --- | ---: |
| `cpython_native.py` | 201 |
| `dataclass_boundary_native.py` | 14 |
| `dataclass_bridges_native.py` | 18 |
| `dataclass_members_native.py` | 17 |
| `dataclass_native.py` | 13 |
| `descriptor_birth_native.py` | 22 |
| `field_native.py` | 47 |
| `generators_native.py` | 74 |
| `slots_native.py` | 35 |
| `type_native.py` | 72 |
| **Total** | **513** |

`test_strict_scenarios.py` then contained a separate parametrized scenario
dispatcher and 18 parser/expectation/runner regressions. Do not add its
file/backend invocations or the historical source/JSON registries below to the
authored-function migration denominator.

At that checkpoint, two original authored tests had migrated into three files:
`migrated_checked_fields_explicit.py`,
`migrated_checked_fields_opted_out_child.py`, and
`migrated_checked_fields_ordinary_base_fallback.py`. The latter two preserve the
separate outcomes split from the old mixed inheritance test. The original test
functions had been removed, not wrapped inside `ok`. That checkpoint alone
did not migrate or validate the other 199 candidates; the subsequent bulk
migration is recorded separately above.

Evidence for that inspection remains under
`work/strict-single-file-audit/post-ruff-f50-v1/`. The `current-` filename prefix
refers to that fixed checkpoint, not today's tree:
`current-authored-tests.json` recorded names, source hashes and counts;
`current-classification.json` recorded classifications and reasons;
`current-summary.json` recorded per-file counts and then-remaining
capability/fixture lists. `refresh-summary.json` verified the unchanged counts
after the diagnostic parametrization and native test rename. Only stdlib
AST/JSON inventory commands ran for this audit, not project tests, checker
publication or benchmarks. Earlier audit receipts in the parent directory and
the historical baseline below are retained unchanged.

## Historical comment-policy verification — 2026-08-28 (PDT)

This checkpoint preceded the themed bulk migration and its repairs. Its gate
and replay results do not validate the later 259-file tree.

The full `just test-all` run completed at **23:48 PDT on 2026-08-27**, using
the selected optimized CPython build. Its retained result was **881 JIT tests
passed, four failed; 11 raw-runtime tests passed; 1004 Python batches passed,
11 failed, with no timeouts**. All 1015 Python batches were reported. Other
workspace Rust test targets passed. The original gate remains recorded as
failed; it was not restarted.

After corrections, **all four original Rust failures and all 11 original
failed Python batches passed targeted replays**. Python replays retained each
complete original group, including passing siblings. The first Rust replay
also exposed an uncleared expected C-API exception in an ordinary-storage
fixture; the corrected exact test then passed. Expectations were updated for
comment selection and field-only enforcement, not replaced with new xfails or
call-boundary checks.

One failure was a real checker defect: an `Any` field declaration was confused
with its runtime namespace default, unnecessarily declining ordinary
dataclasses. Local Ruff commit `f50eb40db2e2119b06aa0e4a15d75409287e82f8`
separates those facts. Its final staged structured cohort passed **111 tests**;
the authenticated integration replays exercised the committed checker through
actual runtime construction. A further **14 focused tests passed**: the
extended dataclass scenario in three modes, two CPython diagnostic cases, and
nine structured worker-evidence cases. Both retired-config rejection tests,
the affected Rust test-target check, and scoped Rust formatting checks passed.

Every gate/replay cohort retained unchanged-input and runtime-identity
postchecks. CPython remains pinned to
`803aea33e25c5cbea452ee92229de3d562aa0575`. The consolidated local receipt is
`work/logs/comment-policy-completed-validation-v1.json`; it binds the original
gate, individual replays, checker cohort, focused follow-up and inventory
receipts by hash. Original failures and intermediate replay evidence remain
available beside it. These results do not claim a fresh green full-gate run,
the then-unimplemented bulk migration, remote availability, or performance
evidence. No optimization or benchmark campaign was run, and nothing was
pushed.

## Historical migration audit — 2026-08-26 (PDT)

This is the pre-comment-policy baseline, not the current remaining count.
Ordinary helpers and per-module/class policy are now supported directly above;
the old “Extension” category therefore overstates what needs format changes.

Baseline: `c798b4342f2b48327c6f39862a093cf0c9a96a86`, before adding this format.
The denominator below is **authored test functions and unittest methods** in
the 22 non-native `tests/test_strict_*.py` files, not parameter-expanded pytest
nodes and not tests introduced with this format.

- **Direct:** existing sources and behavioral assertions can be packaged in
  module sections and independent blocks. Ordinary Python assertions,
  try/except checks and ctypes calls can stay intact inside `ok`.
- **Extension:** preservation needs more than these three directives, such as
  intentionally ordinary imported helpers, alternate/per-module field policy,
  pre-import hooks, external C fixtures, expected import failure or multi-stage
  profile/artifact continuity. Do not silently make those helpers strict or
  discard the assertions.
- **Bespoke:** the subject is checker/publication/helper tooling, manual native
  policy transitions, or an ordinary-only control, rather than an admitted
  source program followed by independent validation.

| File | Total | Direct | Extension | Bespoke |
| --- | ---: | ---: | ---: | ---: |
| `test_strict_annotation_replay.py` | 22 | 15 | 7 | 0 |
| `test_strict_call_context.py` | 8 | 7 | 1 | 0 |
| `test_strict_cell_errors.py` | 1 | 1 | 0 | 0 |
| `test_strict_cell_regions.py` | 1 | 0 | 1 | 0 |
| `test_strict_checked_calls.py` | 3 | 0 | 3 | 0 |
| `test_strict_checked_fields.py` | 20 | 10 | 9 | 1 |
| `test_strict_class_runtime.py` | 31 | 11 | 15 | 5 |
| `test_strict_dataclass_adapters.py` | 25 | 4 | 21 | 0 |
| `test_strict_dataclass_decline.py` | 2 | 0 | 2 | 0 |
| `test_strict_dataclass_nominal_bindings.py` | 11 | 0 | 11 | 0 |
| `test_strict_descriptor_runtime.py` | 6 | 1 | 4 | 1 |
| `test_strict_dict_policy_transition.py` | 4 | 0 | 0 | 4 |
| `test_strict_entry_runtime.py` | 16 | 11 | 5 | 0 |
| `test_strict_framework_fallback.py` | 2 | 0 | 2 | 0 |
| `test_strict_function_boundaries.py` | 68 | 25 | 37 | 6 |
| `test_strict_generator_protocols.py` | 22 | 21 | 1 | 0 |
| `test_strict_import_admission.py` | 26 | 2 | 12 | 12 |
| `test_strict_membership_order.py` | 1 | 1 | 0 | 0 |
| `test_strict_method_dispatch.py` | 6 | 1 | 5 | 0 |
| `test_strict_module_preconditions.py` | 7 | 3 | 4 | 0 |
| `test_strict_nominal_methods.py` | 20 | 2 | 18 | 0 |
| `test_strict_pyperformance_sources.py` | 18 | 0 | 0 | 18 |
| **Total** | **320** | **115** | **158** | **47** |

The ten `test_strict_*_native.py` files contain another **513** authored methods.
They intentionally exercise manual native construction/policy/ABI boundaries or
ordinary CPython controls and should stay separate. Thus, across all 32 baseline
`test_strict_*.py` files: **833 authored tests**, comprising 115 direct
candidates, 158 requiring extensions, and 560 retained bespoke/native tests.
Wrapping a native fixture in an otherwise empty scenario is not a migration
to checker-driven coverage.

### Existing source-file and JSON registries

These are different counting units, overlapping the pytest wrapper families;
**do not add them to the authored-test counts above**.

| Historical source collection | Entries/files then | Candidates at that inspection |
| --- | ---: | ---: |
| Legacy `tests/integration_modules/*.py` delimiter files | 298 | 246 |
| Builtin primitive JSON cases | 4 | 4 |
| Closed pipeline JSON cases | 12 | 0 complete; 12 after splitting ordinary controls |
| Import regression JSON cases | 47 | 32 |
| Module precondition JSON cases | 19 | 0 complete; 19 after splitting ordinary controls |

Of the 246 legacy candidates, 238 retained the original source/validator pair,
five used an already-reviewed strict-rejection validator, and three used an
already-retained frame-free semantic validator. Another seven needed embedded
ordinary controls split out, one needed ordinary dependency support, two expected
import failure, 35 exercised ordinary interoperation plus checker rejection,
five were frame-only XFAILs, one had bad syntax and one remained unreviewed.
The four JSON registries then contained 36 strict-behavior candidates out of
82 entries; 33 preserved the original source/validator pair.

This is a **static, source-reviewed migration assessment**, not evidence that
all candidates have been converted or pass under the new default field policy.
Preserve existing ordinary controls, safety assertions and execution witnesses
when converting. The detailed per-test rationales and source evidence remain
locally under `work/strict-single-file-audit/` (`large.json`, `other.json`,
`native-and-cases.json`, and compact `summary.json`).

An independent guest AST audit verified every authored name and per-file count
against the immutable baseline; its receipt and source/audit hashes are in
`work/strict-single-file-audit/verified-inventory.json`.

## Historical focused verification

- `work/logs/strict-single-file-focused-v1.json`: 46 selected tests passed,
  including the first five files in all three modes and actual subprocess
  regressions for mismatched/missing exceptions, prefix failures, import/checker
  failures, isolation and early zero exits.
- `work/logs/strict-single-file-opt-in-red-v1.json`: four semicolon-header
  regressions reproduced misplaced future imports. AST equivalence alone had
  missed the invalid compilation order in the reused source-preparation helper.
- `work/logs/strict-single-file-opt-in-green-v1.json`: 53 selected tests passed
  after statement-boundary insertion was fixed, including that helper's test
  family and the sixth, package/generator scenario in all three modes. Existing
  source bytes remain intact; the regressions also compile the actual output.

Both successful cohorts passed the selected optimized native runtime's identity
postchecks. The follow-up was scoped to the affected source helper and new
scenario; no new full-suite run, benchmark or push was performed. These results
validate the format and its six examples, not bulk migration of the candidates.
