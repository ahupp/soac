
## Regenerating transform fixtures

If a transform change updates the expected desugaring, regenerate the fixture
outputs with:

```
cargo run --bin regen_snapshots
```

# Development Environment

Install the Python-side venv and the nightly Rust codegen backend used by
`soac-jit`:

```
just setup-dev-env
```

# CLIF

```
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


# Principles

  * Locality: for any specific concept, it's better to handle it in one place.
    e.g, prefer to handle different kinds of load/store (global, nonlocal,
    local, class-body) in one place, rather than spreading them across many
    different transforms.  For example, things we prefer not to do:
      - have many different layers of the system aware of annotations and annotationlib
      - special cases that match on specific internal variable names
      - many different sites aware of scoping rules

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

- `DIET_PYTHON_SPECIALIZATION_MODE=profile|verify|apply`
  Select the runtime specialization phase:
  - `profile`: run unspecialized, instrument specialization input
    counters, and write `<counters-dir>/profile.bin`.
  - `verify`: read `<counters-dir>/profile.bin`, apply its
    specializations, instrument specialization input counters again, and
    write `<counters-dir>/verify.bin`.
  - `apply`: read `<counters-dir>/profile.bin`, apply its
    specializations, and emit no specialization counters.
  Leave unset for the ordinary unspecialized/no-counter path, or when
  using the low-level override environment variables below.

- `DIET_PYTHON_COUNTERS_DIR=/path/to/counters-dir`
  Directory used by `DIET_PYTHON_SPECIALIZATION_MODE`. The runtime
  creates the directory when it writes counters. The conventional files
  are `profile.bin` for the specialization input and `verify.bin` for
  the countered verification pass.

- `DIET_PYTHON_CALL_TARGET_COUNTERS=1`
  In `fn call_target_counter_instrumentation_enabled`, at
  [soac-blockpy/src/passes/trace/mod.rs:27](/home/adam/project/soac-profile/soac-blockpy/src/passes/trace/mod.rs#L27),
  enable runtime call-target profiling. Prefer
  `DIET_PYTHON_SPECIALIZATION_MODE=profile` for normal multi-pass
  specialization runs.

- `DIET_PYTHON_GLOBAL_LOAD_COUNTERS=1`
  In `fn global_load_counter_instrumentation_enabled`, at
  [soac-blockpy/src/passes/trace/mod.rs:19](/home/adam/project/soac-profile/soac-blockpy/src/passes/trace/mod.rs#L19),
  enable global-load profiling counters.

- `DIET_PYTHON_KEY_LAYOUT_COUNTERS=1`
  In `fn key_layout_counter_enabled`, at
  [soac-jit/src/module_type.rs](/home/adam/project/soac-profile/soac-jit/src/module_type.rs),
  record cold key-layout metadata into the counter dump. Module-key
  entries come from the lowered module global-name table; type-key
  entries come from CPython split-key insertion watcher events. This is
  enabled automatically in call-target counter profiling mode; set this
  environment variable when you want key-layout metadata without
  call-target probes.

- `DIET_PYTHON_COUNTERS_OUTPUT_FILE=/path/to/dump.bin`
  In `fn counter_dump_file_from_env`, at
  [soac-jit/src/module_type.rs:570](/home/adam/project/soac-profile/soac-jit/src/module_type.rs#L570),
  write a counter dump on process exit. This is a low-level file
  override; prefer `DIET_PYTHON_COUNTERS_DIR` plus
  `DIET_PYTHON_SPECIALIZATION_MODE=profile|verify` for normal runs.

- `DIET_PYTHON_COUNTERS_FILE=/path/to/dump.bin`
  In `fn load_call_target_specializations`, at
  [soac-jit/src/jit/mod.rs:1968](/home/adam/project/soac-profile/soac-jit/src/jit/mod.rs#L1968),
  read an existing counter dump and derive specializations in-process,
  overriding the mode-derived `<counters-dir>/profile.bin` input. This
  is input-only; it is not rewritten on exit.

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

- `DIET_PYTHON_UNSOUND_INDEXED_STORES=1`
  In `fn unsound_indexed_stores_enabled`, at
  [soac-jit/src/jit/mod.rs](/home/adam/project/soac-profile/soac-jit/src/jit/mod.rs),
  enable raw indexed store fast paths for existing indexed module-global
  and split instance-field slots. This is an intentionally unsound
  performance experiment: it skips CPython dict/object/type watchers,
  dict version updates, insertion-order maintenance, and first-insert
  bookkeeping. Leave unset for correctness tests and ordinary runs.

Notes:
- In normal workflows set one `DIET_PYTHON_COUNTERS_DIR` for the whole
  multi-pass run and change only `DIET_PYTHON_SPECIALIZATION_MODE`.
- The low-level `DIET_PYTHON_CALL_TARGET_COUNTERS=1` profiling path
  still disables specialization loading unless an explicit
  `DIET_PYTHON_SPECIALIZATION_MODE` is set.
- In normal workflows prefer `DIET_PYTHON_COUNTERS_DIR` over wiring
  separate input and output files. Use the file-level env vars for
  inspector/tests/ad-hoc replay.

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
  The default is empty, which runs without CPU pinning. Set an explicit
  CPU core when you want lower scheduler or heterogeneous-core variance.

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
  value disables CPU pinning and is the default.

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
