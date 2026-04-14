# Development Environment

Install the Python-side venv and the nightly Rust codegen backend used by
`soac-jit`:

```
just setup-dev-env
```

`setup-dev-env` also installs the `ruff` command with uv. The repo keeps uv
state under the working tree (`.uv-cache`, `.uv/`, `.xdg/`, and
`soac-module-cache`) and puts the repo-local uv tool bin directory on `PATH`,
so later test and benchmark recipes can run uv in offline mode instead of
fetching through the sandbox.

For jj worktrees, `just setup-dev-env` infers the parent checkout from a
file-backed `.jj/repo` when possible. Set
`SOAC_PARENT_REPO=/path/to/parent/checkout` to override that inference or when
the parent cannot be inferred. The parent checkout owns `bench/` as a regular
directory, and the setup recipe symlinks `vendor/cpython`, `bench/`,
`.uv-cache`, `.uv/`, `.xdg/`, `soac-module-cache`, and `tmp/cargo-home` from
the parent checkout so temporary worktrees can reuse the already-fetched
offline state, BlockPy module cache, and shared benchmark artifacts.

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

## Local Tooling

- `UV_CACHE_DIR`, `UV_TOOL_DIR`, `UV_TOOL_BIN_DIR`, `XDG_CACHE_HOME`,
  `XDG_DATA_HOME`, and `XDG_RUNTIME_DIR`
  The `.envrc` and `Justfile` point these at repo-local directories by default
  so uv package cache, installed tools, and XDG state stay under the working
  tree. The `Justfile` also respects pre-set values for these variables, which
  allows temporary worktrees to use explicit writable shared cache roots.
  `just setup-dev-env` installs `ruff` into the repo-local uv tool bin
  directory.

- `SOAC_PARENT_REPO=/path/to/parent/checkout`
  Optional override for `just setup-dev-env` inside a jj worktree. The recipe
  normally infers the parent checkout from a file-backed `.jj/repo`; the parent
  checkout owns `bench/` as a regular directory, `vendor/cpython`, and the
  shared offline state symlinked into the worktree: `.uv-cache`, `.uv/`,
  `.xdg/`, `soac-module-cache`, and `tmp/cargo-home`.

- `SOAC_MODULE_CACHE_DIR=/path/to/cache`
  Directory for the pre-optimization BlockPy module cache. Cache filenames are
  keyed by the hash of the Python source text plus the SOAC build identity.
  `.envrc` and the `Justfile` default this to `soac-module-cache`, and
  `just setup-dev-env` shares that directory across jj worktrees.

- `SOAC_PRECOMPILED_LIBRARY=/path/to/libsoac_precompiled.so`
  Optional runtime source for offline-precompiled direct function bodies. When
  set, SOAC loads the shared library once, looks up direct-entry symbols by
  module name, source hash, and function id, patches module-constant pointer
  slots for matching modules, and falls back to normal lazy JIT when a function
  symbol is missing.

- `UV_OFFLINE=1`
  Normal test and benchmark recipes set this for uv-backed venv refreshes after
  `setup-dev-env` has populated the repo-local cache and installed tools. Use
  plain `just update-venv` or rerun `just setup-dev-env` when dependency changes
  intentionally require network access.

## Import Hook And Runtime Behavior

SOAC Rust runtime/JIT environment variables are parsed into a typed config once
at the relevant entrypoint. Unset variables use documented defaults; present
typed variables must use recognized values. Boolean knobs accept `1`, `true`,
`yes`, or `on` for true and `0`, `false`, `no`, or `off` for false.

- `SOAC_MODULE_ENABLED=path:/absolute/or/relative/root[,path:/another/root]`
  In `def _module_is_enabled`, at
  [soac_py/src/soac/import_hook.py:39](/home/adam/project/soac-profile/soac_py/src/soac/import_hook.py#L39),
  restrict the import hook to resolved source paths under the listed
  file-tree roots. When unset, an installed import hook attempts to
  transform every transformable Python source import.

- `SOAC_COMPILE_MODE=eager`
  In `fn eager_clif_compile_requested`, at
  [soac-pyo3/src/jit_runtime.rs:96](/home/adam/project/soac-profile/soac-pyo3/src/jit_runtime.rs#L96),
  eagerly compile lazy CLIF/JIT entries as they are registered instead
  of waiting for first execution.

- `SOAC_EXEC_TRACE=<selector>`
  In `fn parse_trace_env`, at
  [soac-blockpy/src/passes/trace/mod.rs:15](/home/adam/project/soac-profile/soac-blockpy/src/passes/trace/mod.rs#L15),
  enable basic-block tracing. Accepted forms are:
  - `all`, `1`, `*`, or empty selector: trace all functions
  - `<exact-qualname>`: trace one function
  - append `:params` to include block parameters

- `SOAC_LOG=<tracing-filter>`
  Controls SOAC Rust diagnostic logging. The filter portion uses
  `tracing-subscriber` syntax, for example `SOAC_LOG=trace` or
  `SOAC_LOG=soac_jit=info,soac_blockpy=trace`. Append
  `;json=/path/to/events.jsonl` to write tracing JSONL to that file
  instead of formatted stderr. Module-load timings are emitted as
  `soac_module_load` tracing events, and JIT-codegen timing is emitted
  through `soac_jit_codegen`. Apply-mode indexed specialization hit and
  fallback summaries are emitted through `soac_specialization_runtime`.
  BlockPy module-cache hits and stores are emitted through
  `soac_blockpy_module_cache`.
  Enable them with
  `SOAC_LOG=soac_module_load=info,soac_jit_codegen=info,soac_specialization_runtime=info`.
  When `SOAC_LOG` is unset and `SOAC_WORK_DIR` is set, SOAC writes
  default JSON events to `$SOAC_WORK_DIR/events.jsonl`, including the
  `soac_specialization_runtime` target.

- `SOAC_PYTEST_TRACE=1`
  Enables the verbose pytest module-load and JIT-codegen JSON trace for
  `just pytest ...` and `just test-all` when `SOAC_LOG` is unset. The trace is
  disabled by default for green correctness runs. It writes to
  `SOAC_PYTEST_EVENTS_LOG` when set, otherwise `logs/pytest_events.jsonl`.

- `SOAC_PYTEST_EVENTS_LOG=/path/to/events.jsonl`
  Overrides the pytest trace output path used when `SOAC_PYTEST_TRACE=1`.

- `SOAC_RUN_SLOW_TESTS=1`
  Includes pytest tests marked `slow` in `just pytest ...` and `just test-all`.
  Slow tests are deselected by default so ordinary green runs skip intentionally
  expensive broad-mode coverage. When invoking pytest directly, pass
  `--run-slow` to include the same tests.

- `SOAC_CRANELIFT_OPT_LEVEL=none|speed|speed_and_size`
  Override the Cranelift optimization level used by the process JIT.
  Normal runtime and benchmark runs default to `speed`. The
  `just pytest`, `just test-all`, and `just run-cpython-tests` recipes default
  this to `none` unless the caller already set it, because correctness tests are
  latency-sensitive and should not spend cold-start time optimizing import-time
  helper code.

- `SOAC_JIT_EMIT_REFCOUNTS=0`
  Disable generated JIT INCREF/DECREF emission by inlining the SOAC runtime
  refcount helpers as no-ops. Refcount emission is enabled by default; only
  `0`, `false`, `no`, or `off` disable it. This is an intentionally unsound
  performance experiment knob and leaks Python references.

## Counters And Specialization

- `SOAC_WORK_DIR=/path/to/work-dir`
  Runtime work directory for generated process-local output. In normal
  specialization workflows this directory contains:
  - `profile.bin`: specialization input recorded by the profile pass.
  - `verify.bin`: countered output recorded by the verify pass.
  - `events.jsonl`: default tracing JSONL when `SOAC_LOG` is not
    set.

- `SOAC_OPT_MODE=none|profile|verify|apply`
  Select the runtime specialization phase:
  - `none`: run the ordinary unspecialized path, do not instrument
    specialization counters, do not read `$SOAC_WORK_DIR/profile.bin`,
    and do not write counter dumps. This is equivalent to leaving
    `SOAC_OPT_MODE` unset, but is useful when a parent environment may
    already set it.
  - `profile`: run unspecialized, instrument specialization input
    counters, and write `$SOAC_WORK_DIR/profile.bin`.
  - `verify`: read `$SOAC_WORK_DIR/profile.bin`, apply its
    specializations, instrument specialization input counters again, and
    write `$SOAC_WORK_DIR/verify.bin`. Verify mode exercises indexed
    store fast paths so their hit/fallback counters measure the
    specialized steady-state path.
  - `apply`: read `$SOAC_WORK_DIR/profile.bin`, apply its
    specializations, and emit no specialization counter dump files.
    When event logging is enabled through `SOAC_LOG` or the default
    `$SOAC_WORK_DIR/events.jsonl`, apply mode still records in-process
    indexed specialization hit/fallback counts long enough to emit
    `soac_specialization_runtime` summary events at module teardown.
  Set `SOAC_WORK_DIR` for any mode that reads or writes counters. Leave
  `SOAC_OPT_MODE` unset, or set it to `none`, for the ordinary
  unspecialized/no-counter path.

Notes:
- In normal workflows set one `SOAC_WORK_DIR` for the whole multi-pass
  run and change only `SOAC_OPT_MODE`.
- `SOAC_ENABLE_PROFILED_COLD_BLOCKS=1` replays `block_entry` counters
  from `$SOAC_WORK_DIR/profile.bin` as Cranelift `cold` block hints in
  `verify`/`apply`. This stays disabled by default; profiling still
  records the underlying `block_entry` counters either way.
- The `apply` phase may emit explicitly marked `BEHAVIOR_CHANGE`
  fast paths. Today that includes raw indexed module-global / instance
  field stores outside module-init code, and undeclared known-builtin
  loads lowered to `RuntimeName` constants.

## IPython Optimization Explorer

Launch the repo-managed IPython with the built SOAC extension available:

```sh
just ipython
```

Load the notebook helper with:

```python
%load_ext soac.ipython
```

Then profile a top-level Python function from the IPython namespace and render
the specialized CLIF or VCode for that function:

```python
def add(a, b):
    return a + b

%soac-profile add(1, 1)
%soac-clif add
%soac-clif-annotate add
%soac-vcode add
```

`%soac-profile` materializes the recovered function source into a temporary
module, runs that module through SOAC with `SOAC_OPT_MODE=profile`, calls the
function with the provided arguments, and records the profile counters for the
session. `%soac-clif` uses `soac-inspector` with those counters to print
specialized Cranelift IR, which is the more readable structured view for most
optimization decisions. `%soac-clif-annotate` sends the recovered Python source,
specialized CLIF, and decoded specialization-counter context to `codex exec` and
prints annotated CLIF with comments describing what each block is doing.
`%soac-vcode` prints the lowered assembly-like VCode when instruction selection
or final code shape matters. This initial explorer supports top-level Python
functions whose source can be recovered by `inspect.getsource`; nested
functions, methods, and functions that depend on non-materialized notebook
globals are out of scope for now.

## Perf And Benchmarking

- `just benchmark`
  The default benchmark recipe runs the transformed profile, verify,
  and specialized apply passes and writes the raw result directory under
  `bench/{change_id}_{commit_id}` for one-off runs or `bench/{change_id}`
  for finalized runs. It always records and prints the actual current `@`
  revision that it executed, so switch revisions first with `jj edit <rev>` if
  you want to benchmark some revision other than the current checkout. By
  default it keeps only the benchmark log and raw
  counter files (`profile.bin`, `verify.bin`, `events.jsonl`); it does
  not run `perf` and it does not build inspector-based counter/CLIF
  artifacts. The specialized apply phase reports both the default
  refcounts-enabled throughput and an additional unsound
  `SOAC_JIT_EMIT_REFCOUNTS=0` diagnostic throughput.

- `just benchmark-deep-profile`
  Run `just benchmark`, then add the heavier follow-on artifacts in the
  same result directory: counter/specialization text dumps, rendered
  specialized CLIF/VCode/CFG, `perf` capture, and perf-annotated VCode.

- `just benchmark-deep-profile-from-profile <result-dir>`
  Start from an existing result directory with `counters/profile.bin`,
  rerun only the verify pass to produce `verify.bin`, then add the same
  deep-profile artifacts without rerunning the profile pass.

- `just precompile-shared-library counters=<profile.bin> out=<lib.so>`
  Offline precompile a counter-referenced set of cached BlockPy modules into
  relocatable object files and link them into a shared library. The counter file
  normally comes from a previous profile pass, and the matching pre-optimization
  BlockPy cache entries must still exist in `SOAC_MODULE_CACHE_DIR`
  (`soac-module-cache` by default). When `counters` is omitted, the recipe uses
  `$LAST_BENCHMARK_COUNTERS`. Set `SOAC_PRECOMPILED_LIBRARY` to the resulting
  `.so` to let runtime direct-function setup use matching precompiled entries.

- `SOAC_JIT_PERF_HELPER_FRAMES=1`
  In `fn should_preserve_perf_helper_frames`, at
  [soac-jit/src/jit/specialized_helpers.rs:1700](/home/adam/project/soac-profile/soac-jit/src/jit/specialized_helpers.rs#L1700),
  select profiling-oriented helper wrappers that preserve explicit stack
  frames. This improves perf call stacks but is slower than the default
  fast helper path. The perf recipes default it on.

- `jit-$PID.dump`
  SOAC always records JIT code-load events on Linux. The dump is written
  to `SOAC_WORK_DIR` when that variable is set, or `/tmp` otherwise.

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

## CPython Test Selection

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
