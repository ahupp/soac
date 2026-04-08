# Plan


Python is a surprisingly complicated language, and to run it fast it first needs
to be made into a smaller language.  There are a few phases here:

python -> python:
  * Strip annotated assignments ("x : int = 1"), and emit as `__annotate__` / `__annotate_func__`.
  * Rewrite "private" names in classes like `__foo` -> `_{classname}_foo`.
  * `assert` -> `if __debug__`
  * `if..elif` into a chain of `if..else`
  * type aliases and parameters into calls to `TypeVar` / `TypeParam` etc.
  * multi-target assign and delete to single target + temporaries
  * f-strings to explicit string formatting
  * augassign and operators -> explicit function calls

python -> bb python
  * flow control: for/while/with



# diet-python

This repository includes a small Rust utility for transforming Python source
code. It parses a file with Ruff's parser and rewrites binary operations and
augmented assignments (e.g., `+=`) into calls to the corresponding functions in
the standard library's `operator` module. The transformation is idempotent, so
re-running it on already rewritten code leaves the output unchanged.


Run it with:

```
cargo run --bin diet-python -- path/to/file.py
```

## Python import hook

To apply the transform automatically when modules are imported, install the
provided import hook:

```python
from soac import import_hook
import_hook.install()
```

After calling `install()`, any subsequent imports will be rewritten using the
`diet-python` transform before execution.

Run the included example to see the hook in action:

```
python example_usage.py
```

The script installs the hook, imports `example_module`, and asserts that its
bytecode calls `operator.add` instead of using `BINARY_OP`.

## Regenerating transform fixtures

If a transform change updates the expected desugaring, regenerate the fixture
outputs with:

```
cargo run --bin regen_snapshots
```

# CLIF

```
$ rustup component add rustc-codegen-cranelift-preview --toolchain nightly
$ ./rust-clif-dist/rustc-clif --out-dir=clif-out/ --crate-type=rlib fastadd.rs -Cdebuginfo=0 --emit link,llvm-ir
```

# Log

2026-01-15:
  - Totals: duration 18m 3s; tests run 37,414; failures 747; skipped 1,706; test files run 483/492; failed 103; env_changed 1;
    skipped 31; resource_denied 9
2026-01-16:
  - Test files: 401 passed / 492 total (483 run; 81 failed; 1 env_changed; 31 skipped; 9 resource_denied).
  - Test cases: 39,237 passed / 39,820 total (583 failed; 1,835 skipped).

Then
• Test File Counts

  - Passing: 388/492
  - Run: 483/492
  - Failed: 95
  - Skipped files: 44

  Individual Test Cases

  - Run: 39,320
  - Passed: 38,685
  - Failed: 635
  - Skipped: 1,754

2026-01-17:
Total duration: 33 min 49 sec
Total tests: run=28,491 failures=612 skipped=1,426
Total test files: run=488/492 failed=160 skipped=24 resource_denied=4
Result: FAILURE


2026-02-02:

Total duration: 48 min 11 sec
Total tests: run=32,863 failures=705 skipped=1,778
Total test files: run=483/491 failed=132 skipped=27 resource_denied=8
Result: FAILURE


# Perf


2026-02-04: First run of transformed interpreter path

transformed interpreter
15880011868 3148 loops/s
stock cpython
967408991 1033688 loops/s
transform-only
1001108600 998892 loops/s

= 328x slower

2026-03-05: Full JIT

- Warmed in-process comparison:
  - JIT: logs/benchmark_jit_warm.log -> 5238 loops/s
  - Stock: logs/benchmark_stock_warm.log -> 824962 loops/s
= 157x slower

Vectorcall entry:

  Relative performance:

  - Stock is 95.71x faster than current JIT path
  - JIT is 1.045% of stock throughput

Use C API for operators:

  Relative:

  - JIT is 1.940% of stock throughput
  - Stock is 51.54x faster on this pystone run

2026-03-08: Remove tuple state passing between blocks:

  - JIT transformed: 23684 loops/s
  - Stock CPython: 913322 loops/s

  Relative:

  - stock is 38.56x faster
  - JIT is 2.59% of stock throughput

2026-03-25:  whole lot of cleanup, no perf work

• Current cold comparison from logs/benchmark-pystone-compare-20260325.log:

  - jit transformed: 30,536 loops/s
  - stock cpython: 906,698 loops/s

  Relative:

  - Stock is 29.69x faster
  - JIT transformed is 3.37% of stock throughput


2026-04-03:

changes:
  - refcounting as cranelift functions, constant pool for all strings
    - JIT/transformed: 105,083 loops/s
    - Stock CPython: 830,761 loops/s
    - transformed is about 0.126x stock, so stock is 7.9x faster.

  - 40e43654 Use Cranelift speed opt level and native ISA for JIT benchmarks
      - transformed/JIT: 91,257 loops/s
      - stock CPython: 754,886 loops/s
      - transformed is 0.121x stock, so stock is about 8.27x faster
      - timing: real 10.75, user 14.99, sys 0.83
      - log: logs/benchmark_opt_native_20260403.log
  - 404cbee4 Inline runtime CLIF support helpers into JIT callers
      - transformed/JIT: 119,398 loops/s
      - stock CPython: 739,834 loops/s
      - transformed is 0.161x stock, so stock is about 6.20x faster
      - timing: real 9.54, user 14.81, sys 0.76
      - log: logs/benchmark_opt_native_inlining_20260403.log
  - lift runtime functions to constants, immortal constants
      - transformed/JIT: 175,380 loops/s
      - stock CPython: 759,045 loops/s
      - transformed is about 0.231x stock
      - stock is about 4.33x faster
  - write through globals cache
      - transformed/JIT: 177,856 loops/s
      - stock CPython: 745,030 loops/s
      Relative performance:  transformed is about 0.239x stock stock is about 4.
  - really use vectorcall
      - transformed/JIT: 221,433 loops/s
      - stock CPython: 892,476 loops/s
      - transformed is about 0.248x stock, so stock is about 4.03x faster
  - fix read through globals cache, near 100% hitrate
      - JIT transformed: 245,183 loops/s
      - Stock CPython: 954,347 loops/s
      - So without counters enabled, the JIT is about 25.7% of stock throughput, or 3.89x slower.

2026-04-06: better specialization coverage, and constant string interning:
  - transformed/JIT profile pass: 221,542 loops/s
  - transformed/JIT specialized pass: 294,980 loops/s
  - stock CPython: 864,134 loops/s

  Headline comparison:

  - specialized transformed is about 0.341x stock
  - stock CPython is about 2.93x faster

# Design

Dropping to basic block format:

 - gave control over name binding for functions
 - significantly improved fidelity to flow control, made generators easier, and reduced JIT surface area


 # Principles

  * Locality: for any specific concept, it's better to handle it in one place.
    e.g, prefer to handle different kinds of load/store (global, nonlocal,
    local, class-body) in one place, rather than spreading them across many
    different transforms.  For example, things we prefer not to do:
      - have many different layers of the system aware of annotations and annotationlib
      - special cases that match on specific internal variable names
      - many different sites aware of scoping rules
  *


# Optimizations

 * Inlining
   * Is there only one caller?
   * Is it below < size?
   * Does it unlock other optimizations?
 * Specialization
    * Known cell address
    * Call fastpath knowing exact sig of target
    * Unboxing

 * Can we skip deleted checks on this value?

 * Minimize refcounting
 * Code size/locality
 * Maximize register use
 * Avoid constant exception checking
 * Flow control exceptions to jumps
 * Compile-time computation
 * Known subclasses
   * No overrides to function
 * Type hints enforcement
 * Escape analysis, stack allocate
   * Inline closure cells
 * Green threads for async

## Facts
 * Constant
 * ReadOnly
 * ExactTypes(...)

# Environment Variables

This repo consults a number of environment variables directly. The list
below is the user-facing set that changes runtime behavior, profiling,
benchmarking, test wrappers, or the local web UI. Pure `Justfile`
plumbing such as `REPO_ROOT`, `VENV_DIR`, `WEB_DIR`, and similar helper
exports are intentionally omitted here.

## Import Hook And Runtime Behavior

- `DIET_PYTHON_INSTALL_HOOK=1`
  Have the repo-root [`sitecustomize.py`](/home/adam/project/soac-profile/sitecustomize.py)
  install the transformed import hook automatically at interpreter
  startup.

- `DIET_PYTHON_INTEGRATION_ONLY=1`
  In `def _integration_only_enabled`, at
  [soac_py/src/soac/import_hook.py:18](/home/adam/project/soac-profile/soac_py/src/soac/import_hook.py#L18),
  restrict transformed imports to integration-test modules instead of
  transforming arbitrary imports.

- `DIET_PYTHON_ALLOW_TEMP=1`
  In `def _should_transform`, at
  [soac_py/src/soac/import_hook.py:57](/home/adam/project/soac-profile/soac_py/src/soac/import_hook.py#L57),
  allow the import hook to transform modules loaded from the system temp
  directory. By default temp files are skipped.

- `DIET_PYTHON_JIT_COMPILE_MODE=eager`
  In `fn eager_clif_compile_requested`, at
  [soac-pyo3/src/jit_runtime.rs:96](/home/adam/project/soac-profile/soac-pyo3/src/jit_runtime.rs#L96),
  eagerly compile lazy CLIF/JIT entries as they are registered instead
  of waiting for first execution.

- `DIET_PYTHON_BB_TRACE=<selector>`
  In `fn parse_trace_env`, at
  [soac-blockpy/src/passes/trace/mod.rs:15](/home/adam/project/soac-profile/soac-blockpy/src/passes/trace/mod.rs#L15),
  enable basic-block tracing. Accepted forms are:
  - `all`, `1`, `*`, or empty selector: trace all functions
  - `<exact-qualname>`: trace one function
  - append `:params` to include block parameters

- `DIET_PYTHON_DEBUG_DIRECT_METHOD_SPECIALIZATIONS=1`
  In `unsafe fn register_owner_types_from_type`, at
  [soac-jit/src/lib.rs:912](/home/adam/project/soac-profile/soac-jit/src/lib.rs#L912),
  print debug logging for direct-method and owner-type specialization
  registration.

## Counters And Specialization

- `DIET_PYTHON_CALL_TARGET_COUNTERS=1`
  In `fn call_target_counter_instrumentation_enabled`, at
  [soac-blockpy/src/passes/trace/mod.rs:27](/home/adam/project/soac-profile/soac-blockpy/src/passes/trace/mod.rs#L27),
  enable runtime call-target profiling. This is the first-pass input for
  call-target and operator specialization.

- `DIET_PYTHON_GLOBAL_LOAD_COUNTERS=1`
  In `fn global_load_counter_instrumentation_enabled`, at
  [soac-blockpy/src/passes/trace/mod.rs:19](/home/adam/project/soac-profile/soac-blockpy/src/passes/trace/mod.rs#L19),
  enable global-load profiling counters.

- `DIET_PYTHON_COUNTERS_OUTPUT_FILE=/path/to/dump.bin`
  In `fn counter_dump_file_from_env`, at
  [soac-jit/src/module_type.rs:570](/home/adam/project/soac-profile/soac-jit/src/module_type.rs#L570),
  write a counter dump on process exit.

- `DIET_PYTHON_COUNTERS_FILE=/path/to/dump.bin`
  In `fn load_call_target_specializations`, at
  [soac-jit/src/jit/mod.rs:1968](/home/adam/project/soac-profile/soac-jit/src/jit/mod.rs#L1968),
  read an existing counter dump and derive specializations in-process.
  This is input-only; it is not rewritten on exit.

- `DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS=...`
  In `fn parse_call_target_specializations_env`, at
  [soac-jit/src/jit/mod.rs:1888](/home/adam/project/soac-profile/soac-jit/src/jit/mod.rs#L1888),
  provide an explicit advanced override for call-target
  specializations. If this is set, it wins over
  `DIET_PYTHON_COUNTERS_FILE`.

- `DIET_PYTHON_OPERATOR_SPECIALIZATIONS=...`
  In `fn parse_operator_specializations_env`, at
  [soac-jit/src/jit/mod.rs:1989](/home/adam/project/soac-profile/soac-jit/src/jit/mod.rs#L1989),
  provide an explicit advanced override for operator specializations. If
  this is set, it wins over `DIET_PYTHON_COUNTERS_FILE`.

Notes:
- If `DIET_PYTHON_CALL_TARGET_COUNTERS` is enabled for the current run,
  specialization loading is disabled for that run and the process stays
  in profiling mode.
- In normal workflows you should prefer `DIET_PYTHON_COUNTERS_OUTPUT_FILE`
  for the profiling pass and `DIET_PYTHON_COUNTERS_FILE` for the
  specialized pass rather than manually building specialization strings.

## Perf And Benchmarking

- `SOAC_JIT_PERF_HELPER_FRAMES=1`
  In `fn should_preserve_perf_helper_frames`, at
  [soac-jit/src/jit/specialized_helpers.rs:1700](/home/adam/project/soac-profile/soac-jit/src/jit/specialized_helpers.rs#L1700),
  select profiling-oriented helper wrappers that preserve explicit stack
  frames. This improves perf call stacks but is slower than the default
  fast helper path. The perf recipes default it on.

- `SOAC_JIT_JITDUMP_DIR=/path/to/dir`
  In `fn new`, at
  [soac-jit/src/jit/jitdump.rs:98](/home/adam/project/soac-profile/soac-jit/src/jit/jitdump.rs#L98),
  choose where `soac-jit` writes `jit-$PID.dump`.

- `PERF_BUILDID_DIR=/path/to/dir`
  Used by the perf recipes in [Justfile](/home/adam/project/soac-profile/Justfile)
  and checked in `fn serialize_unwind_info`, at
  [soac-jit/src/jit/jitdump.rs:262](/home/adam/project/soac-profile/soac-jit/src/jit/jitdump.rs#L262),
  to control where perf build-id artifacts are written.

- `WARMUP_LOOPS=<int>`
  In recipe `perf-pystone-jit-warm`, at
  [Justfile:271](/home/adam/project/soac-profile/Justfile#L271), and the
  benchmark recipes near [Justfile:711](/home/adam/project/soac-profile/Justfile#L711),
  control the pre-measurement pystone warmup count.

- `BENCHMARK_CPU=<int>`
  In [scripts/run_benchmark_with_cpu_mode.sh](/home/adam/project/soac-profile/scripts/run_benchmark_with_cpu_mode.sh),
  choose the CPU core that the benchmark recipes pin to with `taskset`.
  The default is `0`. Pinning all benchmark phases to one core reduces
  scheduler and heterogeneous-core variance without requiring privileged
  clock controls.

- `BENCHMARK_CONSTANT_CLOCKS=0|1`
  In [scripts/run_benchmark_with_cpu_mode.sh](/home/adam/project/soac-profile/scripts/run_benchmark_with_cpu_mode.sh),
  control whether the benchmark wrapper temporarily forces steadier CPU clocks for
  the selected benchmark core and its related CPUs by setting the
  governor to `performance`, locking `scaling_min_freq` and
  `scaling_max_freq` to the hardware max frequency, and disabling boost
  when the kernel exposes a `boost` knob. The benchmark recipes default
  this to `0`, and the wrapper restores previous settings on exit when
  constant-clock mode is enabled. When direct writes are not permitted,
  the wrapper uses `sudo` automatically for the sysfs write and restore
  path. Set `BENCHMARK_CONSTANT_CLOCKS=1` to opt in.

- `SPECIALIZATION_PROFILE_LOOPS=<int>`
  In recipe `perf-pystone-jit-specialized`, at
  [Justfile:480](/home/adam/project/soac-profile/Justfile#L480), control
  the first-pass profiling loop count used to derive specializations.

- `PERF_FREQUENCY=<int>`
  In recipe `perf-pystone-jit-warm`, at
  [Justfile:272](/home/adam/project/soac-profile/Justfile#L272), set the
  `perf record -F` sample frequency.

- `PERF_CALL_GRAPH=<mode>`
  In recipe `perf-pystone-jit-warm`, at
  [Justfile:273](/home/adam/project/soac-profile/Justfile#L273), set the
  `perf record --call-graph` mode. The default is `dwarf,65528`, which
  captures a much larger user-space stack dump so mixed JIT/CPython
  stacks are less likely to truncate into misleading leaf-only C helper
  frames.

- `PERF_PERCENT_LIMIT=<float>`
  In recipe `perf-pystone-jit-warm`, at
  [Justfile:274](/home/adam/project/soac-profile/Justfile#L274), control
  the threshold used when rendering perf text reports.

## Resource Limits

- `DIET_PYTHON_MEMORY_LIMIT_MB=<int>`
  In [scripts/run_with_limits.sh](/home/adam/project/soac-profile/scripts/run_with_limits.sh),
  set the cgroup memory cap in MiB for recipes that intentionally call
  the limit wrapper, such as `run-cpython-tests`. `0` disables the
  memory cap.

- `DIET_PYTHON_TIMEOUT_SECS=<int>`
  In [scripts/run_with_limits.sh](/home/adam/project/soac-profile/scripts/run_with_limits.sh),
  set the cgroup wall-clock timeout in seconds for limit-wrapper runs.
  `0` disables the timeout.

- `DIET_PYTHON_CPUSET=<cpuset>`
  In [scripts/run_with_limits.sh](/home/adam/project/soac-profile/scripts/run_with_limits.sh),
  restrict a limit-wrapper run to a Linux cpuset such as `0-7`. An empty
  value disables CPU pinning.

- `DIET_PYTHON_SYSTEMD_RUNTIME_DIR=/run/user/<uid>`
  In [scripts/run_with_limits.sh](/home/adam/project/soac-profile/scripts/run_with_limits.sh),
  override the runtime dir used to reach the user systemd bus.

- `SKIP_EXPECTED_FAILURES=1`
  In [scripts/collect_cpython_skip_ids.sh](/home/adam/project/soac-profile/scripts/collect_cpython_skip_ids.sh),
  include expected-failure IDs when building the CPython skip list. Set
  it to `0` to stop filtering on `EXPECTED_FAILURE.md`.

- `CPYTHON_TEST_SETS_GLOB=<glob>`
  In [scripts/run_cpython_test_sets.sh](/home/adam/project/soac-profile/scripts/run_cpython_test_sets.sh),
  choose which test-set files to run.

- `CPYTHON_TEST_TEMPDIR=/tmp/...`
  In [scripts/run_cpython_test_sets.sh](/home/adam/project/soac-profile/scripts/run_cpython_test_sets.sh),
  choose the tempdir used for CPython regrtest set runs.

- `CPYTHON_TEST_LOG_DIR=/path/to/logs`
  In [scripts/run_cpython_test_sets.sh](/home/adam/project/soac-profile/scripts/run_cpython_test_sets.sh),
  choose where per-set CPython logs are written.

- `SKIP_FILE=/path/to/cpython_skipped_tests.txt`
  In [scripts/collect_cpython_skip_ids.sh](/home/adam/project/soac-profile/scripts/collect_cpython_skip_ids.sh),
  choose the base skipped-test list file.

- `EXPECTED_FAILURES_FILE=/path/to/EXPECTED_FAILURE.md`
  In [scripts/collect_cpython_skip_ids.sh](/home/adam/project/soac-profile/scripts/collect_cpython_skip_ids.sh),
  choose the markdown file that contributes expected-failure test IDs.

- `PYTHON_BIN=/path/to/python`
  In [scripts/collect_cpython_skip_ids.sh](/home/adam/project/soac-profile/scripts/collect_cpython_skip_ids.sh),
  choose which Python binary is used when collecting skip IDs.

## Local Web Inspector

- `HOST=<bind-address>`
  In [`fn main`, at [soac-inspector/src/main.rs:8](/home/adam/project/soac-profile/soac-inspector/src/main.rs#L8)],
  control the bind address for the local inspector server. The `Justfile`
  default is `127.0.0.1`.

- `PORT=<port>`
  In [`fn main`, at [soac-inspector/src/main.rs:9](/home/adam/project/soac-profile/soac-inspector/src/main.rs#L9)],
  control the bind port for the local inspector server. The `Justfile`
  default is `8000`.
