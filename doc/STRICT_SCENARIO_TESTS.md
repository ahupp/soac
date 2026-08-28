# Single-file strict integration scenarios

A file under `tests/strict_scenarios/` describes one source-level scenario.
`tests/test_strict_scenarios.py` collects one test per file and execution mode:
compiled SOAC, entry-interpreted SOAC, and strict CPython. It uses the actual
vendored checker and authenticated native startup, not manually installed facts.

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
```

- `# module:name` starts a Python source module. All module sections precede
  test blocks. Multiple modules can import each other using their declared
  names. The first module supplies the bare names used by validation blocks.
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
  Missing/empty blocks, duplicate modules and malformed directives are errors.

In the last block above, a `TypeError` from `A()` is a **failure**. Only the
assignment can satisfy the expectation. Semicolon-separated statements are
distinct statements too. A compound statement such as `with` or `if` counts
as one top-level statement; prefer a final simple operation when that is the
precise boundary being checked. Syntax errors are setup failures, never matched
runtime exceptions.

There is no validator-function convention inside a block: its statements
execute directly. Defining a function does not implicitly call it.

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
just pytest-fast --require-batch-runner 'tests/test_strict_scenarios.py::test_strict_scenario[field_assignment-soac]'
```

Use the documented Lima launcher from the host. Logs, source projects, signed
publications and per-block subprocess output are retained by the existing
`StrictProject` helper.

## Current migration audit — 2026-08-28 (PDT)

A guest AST inventory rechecked at **00:31 PDT on 2026-08-28** counted
**324 authored tests**
in the 22 regular `test_strict_*.py` files: 320 functions and four unittest
methods. This is after two original authored checked-field tests were migrated
into three scenario files and after field-write/policy reconciliation. It counts
neither backend invocations nor case-parameter expansion. One authored function
can cover several source variants and therefore require several scenario files.

Of these, **250 are non-native strict source-integration cases**.
**199 are candidates for the current format**: 86 straightforward source and
validator extractions, plus 113 whose ordinary helpers, source-policy choices
or backend-witness bookkeeping are now expressible. **51 still need additional
runner capabilities or a separately reviewed split.** These are static
source-review counts, not conversion completion or passing-test evidence.

The other 74 regular-file tests are counted separately: **34 ordinary-only
controls, 10 native-ABI probes, nine external-C-fixture consumers and 21
checker/benchmark preparation or worker-tooling tests**. Six of the ten ABI
probes and seven of the nine C-fixture consumers do use actual checker
publication; their additional native subject/setup is why they are separate,
not a claim that they bypass analysis. The two remaining C-fixture consumers
are ordinary controls. Ordinary-only scenarios are allowed, but moving a
control does not add a strict source-integration case.

In the table, **Ready** and **Ordinary/policy** both fit the current format.
The latter records formerly blocked ordinary setup, field-policy or witness
work, rather than a proposed new directive. **Other** identifies separate test
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

### Remaining capabilities and boundaries

The 51 non-native source cases have these requirements; a mixed case can
appear in more than one requirement count:

- **20** retain profile/apply/verify state, counters or optimization-plan
  artifacts. Plain unprofiled execution would lose the assertion. This audit
  does not resume optimization or benchmark work.
- **11** have a genuinely backend-specific subject: exact native source-frame,
  closure/refcount/implicit-cleanup observations, or an escaped SOAC private
  execution handle. This needs explicit backend enrollment or a reviewed
  semantic split, not an empty guarded block. A native entry witness alone is
  not a blocker.
- **Seven** intentionally fail a selected initializer, including one of the
  profile cases; **two** expect checker rejection. Imports, analysis and
  authentication must still succeed outside a runtime `raise` expectation.
- **Two** mutate published source/artifacts before admission; **one** inspects
  emitted signature facts; **three** observe loader/module states before body
  execution. These need explicit publication/admission-phase access.
- **Four** require another interpreter environment, source-tree/path-selection
  layout, or background execution with its logs and timeout controls.
- **Two** require lifetime-neutral module validation. The current runner keeps
  module wrappers alive in its precheck map and `_execute_block` argument, then
  imports them for postchecks. Deleting a copied validator global cannot prove
  that the actual wrapper was collected.

The nine external-C-fixture consumers additionally require their real compiled
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

There are separately **513 authored methods** in the ten native files:

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

`test_strict_scenarios.py` is another separate layer: one parametrized scenario
dispatcher and 18 parser/expectation/runner regressions. Do not add its
file/backend invocations or the historical source/JSON registries below to the
authored-function migration denominator.

Two original authored tests were migrated into three scenario files:
`migrated_checked_fields_explicit.py`,
`migrated_checked_fields_opted_out_child.py`, and
`migrated_checked_fields_ordinary_base_fallback.py`. The latter two preserve the
separate outcomes split from the old mixed inheritance test. The original test
functions were removed, not wrapped inside `ok`; the 199 remaining candidates
are not thereby migrated or validated.

Current evidence is under
`work/strict-single-file-audit/post-ruff-f50-v1/`:
`current-authored-tests.json` records fresh names, source hashes and counts;
`current-classification.json` records each current classification and reason;
`current-summary.json` contains compact per-file counts and exact remaining
capability/fixture lists. `refresh-summary.json` verifies the unchanged counts
after the diagnostic parametrization and native test rename. Only stdlib
AST/JSON inventory commands ran for this audit, not project tests, checker
publication or benchmarks. Earlier audit receipts in the parent directory and
the historical baseline below are retained unchanged.

## Comment-policy verification — 2026-08-28 (PDT)

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
bulk migration of the remaining cases, remote availability, or performance
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

| Source collection | Total entries/files | Current strict-behavior candidates |
| --- | ---: | ---: |
| Legacy `tests/integration_modules/*.py` delimiter files | 298 | 246 |
| Builtin primitive JSON cases | 4 | 4 |
| Closed pipeline JSON cases | 12 | 0 complete; 12 after splitting ordinary controls |
| Import regression JSON cases | 47 | 32 |
| Module precondition JSON cases | 19 | 0 complete; 19 after splitting ordinary controls |

Of the 246 legacy candidates, 238 retain the original source/validator pair,
five use an already-reviewed strict-rejection validator, and three use an
already-retained frame-free semantic validator. Another seven need embedded
ordinary controls split out, one needs ordinary dependency support, two expect
import failure, 35 exercise ordinary interoperation plus checker rejection,
five are frame-only XFAILs, one has bad syntax and one remains unreviewed.
The four JSON registries contain 36 current-strict-behavior candidates out of
82 entries; 33 preserve the original source/validator pair.

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
