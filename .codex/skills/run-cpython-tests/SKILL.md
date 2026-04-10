---
name: run-cpython-tests
description: Run the CPython regression test suite and generate structured summaries of the failures
---

# Run CPython tests

## Run the partitioned fast suite

- Run the checked-in fast test partitions sequentially.
- Use an explicit tempdir under `/tmp` in sandboxed environments.

```bash
./scripts/run_cpython_test_sets.sh --tempdir /tmp/soac-cpython-fast-tests
```

This writes a summary to `logs/cpython_jit_test_sets_summary.log` and one
`logs/cpython_jit_cpython_fast_tests_part_*.log` file per partition.

## Run one CPython test file

```bash
mkdir -p logs
set -o pipefail
just run-cpython-tests 0 -x slow --tempdir /tmp/soac-cpython-single -f /abs/path/to/test_file.py 2>&1 | tee logs/cpython_single_test_file.log
```

`just run-cpython-tests` builds the extension, installs the local package in the
venv, and executes vendored CPython regrtest through `python -m soac.import_hook
test.__main__`. Pass an absolute `-f` file path.

## Run arbitrary regrtest arguments

```bash
mkdir -p logs
set -o pipefail
just run-cpython-tests 0 --tempdir /tmp/soac-cpython-tests -x slow 2>&1 | tee logs/cpython_full_test_run.log
```

Override SOAC environment variables only when explicitly comparing modes.

## Summarize failures from the log

- Locate failure anchors:

```bash
rg -n "^(FAIL|ERROR|TIMEOUT|CRASHED|INTERRUPTED|LEAKED|ENV_CHANGED):" logs/cpython_full_test_run.log
```

- Extract each failure block (look for the separator lines of ===) and classify the failure based on the contents of the error.
- Use these categories in the summary:
  - FAIL: assertion mismatch or explicit test failure; call out the mismatched expectation.
  - ERROR: unexpected exception; report the exception type and message.
  - TIMEOUT: test exceeded 180 seconds; call out the hang/timeout.
  - CRASHED/FATAL: interpreter crash; mention the fatal error or signal.
  - LEAKED/ENV_CHANGED: resource leak or environment mutation; mention the resource.
- Summarize each failing test as: `test_name: <category> - <short reason from output>`.
- If a failure is due to changes to tracebacks / source transforms /
  bytecode, add it to EXPECTED_FAILURES.md with a short explanation
- Otherwise, add it to FAILURES_TO_TRIAGE.md
