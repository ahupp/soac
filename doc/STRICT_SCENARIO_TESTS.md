# Single-file strict integration scenarios

A file under `tests/strict_scenarios/` describes one source-level scenario.
`tests/test_strict_scenarios.py` collects one test per file and execution mode:
compiled SOAC, entry-interpreted SOAC, and strict CPython. It uses the actual
vendored checker and authenticated native startup, not manually installed facts.

## Format

```python
# module:mod1

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
- Every declared module is strict, with the project field policy
  `checked_fields = "supported_annotations"`. The adapter explicitly inserts
  `from __future__ import strict` where absent, preserving docstrings, existing
  future imports and source semantics. It then passes these real source files
  to the existing checker helper; no facts or native authority are fabricated.
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

## Isolation and authentication

The adapter analyzes and publishes the whole file's module set **once per
file/mode run**. It then starts a fresh authenticated interpreter for **each
block**, imports all declared modules, and checks native module witnesses before
running validation. Mutable objects, module caches, builtins changes and test
locals from one block do not carry into the next block.

Validation is ordinary Python, outside the analyzed/sealed modules. It receives
a copy of the first module's namespace and an explicit `module` reference.
Consequently, `answer = 2` changes a validation local, whereas
`module.answer = 2` attempts to change the actual sealed module. Classes and
objects referenced from that copy are the real admitted objects.

Plain, undecorated synchronous module functions receive automatic actual-entry
witnesses. Generator factories, decorated functions and methods are not guessed
to be synchronous entries; cases needing their specific execution-path evidence
must retain appropriate explicit witness assertions in an `ok` block.
The existing `__dp_integration_mode__`, `__dp_integration_soac__` and
`__dp_integration_entry__` validation flags are available.

Parsing, offline-checker errors, startup authentication, module imports, native
witnesses and block-prefix exceptions cannot satisfy `raise`. A completion
receipt rejects a process that exits successfully without finishing validation.
Block failures are retained and later blocks still run in fresh processes;
analysis/publication failure stops the scenario before any blocks run.

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

## Migration audit — 2026-08-26 (PDT)

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

## Focused verification

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
