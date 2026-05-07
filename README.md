# Repository Components

## Rust Crates

- `build_support`
  Shared build-script helpers for computing the SOAC build identity and linking
  against the vendored CPython library. Crates with native Python or inspector
  build steps use it from `build.rs`.

- `soac-config`
  Central typed configuration for SOAC environment variables, logging, work
  directories, optimization modes, and runtime feature flags. Runtime, JIT,
  inspector, and benchmark entrypoints should parse environment state through
  this crate instead of re-reading raw variables.

- `soac-core`
  Core data model crate for BlockPy IR, profile/counter dump formats, runtime
  function IDs, and structured pretty-printing helpers. It owns the serialized
  shapes that are shared between lowering, optimization, JIT, and inspection.

- `soac-cpython`
  Embedding and test-support utilities for the vendored CPython build. It
  locates the repository, configures Python home/search paths, stages extension
  modules for tests, and initializes Python for Rust-side integration tests.

- `soac-driver`
  Codegen-preparation orchestration layered on top of `soac-lowering`. It owns
  pre-optimization module-cache lookup/store, prepared codegen facts,
  env-configured instrumentation, and cache path/metadata helpers used by the
  runtime and optimizer.

- `soac-inspector`
  Local web inspector and command-line tooling for lowering, counter dumps,
  optimization plans, CLIF/VCode rendering, and benchmark artifact analysis. It
  is the main crate for interactive and offline inspection workflows.

- `soac-jit`
  CPython-facing JIT implementation that turns resolved BlockPy functions into
  Cranelift code, manages module runtime state, counters, constants, guards,
  deopt paths, and optional precompiled function loading.

- `soac-jit-runtime`
  Standalone, ABI-shaped runtime helper crate intended to be compiled to CLIF
  and inlined into generated code. It stays close to raw CPython layouts and
  keeps hot helper behavior visible to codegen review.

- `soac-lowering`
  Parser-to-BlockPy lowering pipeline, transformation passes, pass tracking,
  source fixtures, validation, and structured render helpers. It is where raw
  Python syntax is turned into progressively more resolved compiler IR, and it
  owns the public lowering entrypoints.

- `soac-macros`
  Procedural macros used by the lowering and IR layers to reduce repetitive
  enum delegation and match boilerplate. It should stay mechanical and avoid
  owning compiler semantics.

- `soac-opt`
  Optimization planning from profile evidence and cached BlockPy modules. It
  owns v3 plan/emission data, specialization decisions, and
  optimization-family status reporting.

- `soac-pyo3`
  The `_soac_ext` Python extension module. It bridges CPython import-hook
  callbacks into SOAC lowering, JIT module creation, and runtime execution.

- `bench/pystone-rust`
  Rust pystone benchmark package used for baseline or comparison work around
  the Python pystone benchmark. It is outside the main SOAC workspace members.

## Codex Skills

- `analyze-pystone-perf`
  End-to-end pystone performance-analysis workflow that combines benchmark
  counters, perf capture, specialized CLIF rendering, and ranked optimization
  suggestions.

- `benchmark-compare`
  Compares two SOAC pystone benchmark result directories, creating missing
  one-off benchmark artifacts when needed and checking throughput, counters,
  and specialized CLIF differences.

- `fix-test-case`
  Focused workflow for one failing test: reproduce it, add a minimal regression
  under `tests/`, identify the root cause, implement the fix, and rerun the
  narrow check.

- `inspect-pass`
  Opens the local web inspector at a named tracked lowering pass for a concrete
  source example, useful for visually comparing transform stages.

- `python-debug`
  Uses `pdb` to step through Python scripts, continue to exceptions, and
  inspect runtime state at specific lines.

- `python-monitoring-trace`
  Uses `sys.monitoring` to trace selected Python execution events with
  include/exclude controls and optional log output.

- `run-cpython-tests`
  Runs vendored CPython regression tests through SOAC's import-hook path and
  writes structured logs for full, partitioned, or single-file regrtest runs.

- `soac-annotate`
  Profiles a small Python snippet, collects post-opt-v3 BlockPy, specialized
  CLIF, and VCode views, and prepares annotation context for explaining
  generated blocks, guards, counters, and helper calls. It defaults to the
  post-opt-v3 view unless CLIF or VCode is requested.

- `soac-profile-benchmark`
  Runs the SOAC pystone profile/verify/apply benchmark workflow and summarizes
  the generated `work/bench/` result directory.

- `summarize-cpython-failures`
  Summarizes CPython regrtest logs, computes file and test-case totals, and
  groups failures by likely root cause.

## CLI Inspection Tools

Most command-line inspection tools live in `soac_inspector` and can be run as
`cargo run -p soac_inspector --bin <tool> -- ...`. The offline optimization
planner lives in `soac-opt`.

- `soac_inspector`
  Starts the local web inspector server. It serves the interactive pass,
  BlockPy, CLIF, and typed-instruction views, binding to `HOST`/`PORT` or
  `0.0.0.0:8000` by default.

- `list_jit_functions`
  Prints the packed runtime function ID and qualified name for each lowered JIT
  function in a source file. Use this before rendering CLIF or typed
  instructions for a specific function.

- `render_jit_clif`
  Renders generated CLIF for one source file and function ID. It can render
  specialized apply-mode code, pre-inline CLIF, debug plans, CFG dot output,
  and lowered VCode.

- `render_instr_typed`
  Renders typed instruction-level JIT output for one source file and function
  ID, with the same specialized apply-mode module identity handling as
  `render_jit_clif`.

- `inspect_counters`
  Reads a `profile.bin` or `verify.bin` counter dump and prints counters,
  key-layout rows, specialization summaries, or JSON.

- `precompile_blockpy`
  Uses profile counters and cached BlockPy modules to compile referenced
  modules into object files and link an offline shared library for
  `SOAC_PRECOMPILED_LIBRARY`.

- `annotate_cranelift_perf`
  Correlates perf samples with SOAC JIT basic-block maps for a benchmark result
  directory, writes annotated VCode files, and prints block-level sample rows.

# Setup

```
$ cargo install --locked just
$ just setup-dev-env
```

The workflows in `AGENTS.md` depend on using `jj-vcs` for version control and may not work well with codex 
with a regular git repo.

`setup-dev-env` reuses an already-installed nightly Rust toolchain and Cranelift
codegen component rather than upgrading them on every run, because a nightly
refresh forces rebuilds. It also installs the `ruff` command with uv. The repo
keeps uv, XDG, and cargo state under the working tree (`.uv-cache`, `.uv/`,
`.xdg/`, `work/tmp/`, and `work/cargo-home`) and puts the repo-local uv tool bin directory on
`PATH`, so later test and benchmark recipes can run uv in offline mode instead
of fetching through the sandbox.

For jj worktrees, `just setup-dev-env` infers the parent checkout from a
file-backed `.jj/repo` when possible. Set
`SOAC_PARENT_REPO=/path/to/parent/checkout` to override that inference or when
the parent cannot be inferred. The parent checkout owns `work/` as a regular
artifact directory, and the setup recipe symlinks `vendor/cpython`, `work/`,
`.uv-cache`, `.uv/`, `.xdg/`, and `work/cargo-home` from the parent checkout so
temporary worktrees can reuse the already-fetched offline state and shared
benchmark artifacts.

## Documentation Site

The Markdown files under `doc/` can be rendered as a local Astro Starlight
site:

```
$ just docs-install
$ just docs-build
$ just docs-serve
```

`docs-install` installs the Node dependencies declared in `package.json`.
`docs-build` writes the generated site to ignored `work/docs-site/`.
`docs-serve` serves it on `0.0.0.0:8001` by default; pass a port to override
it, for example `just docs-serve 9000`.



# Environment Variables

This repo consults a number of environment variables directly. The list
below is the user-facing set that changes runtime behavior, profiling,
benchmarking, test wrappers, or the local web UI. Pure `Justfile`
plumbing such as `REPO_ROOT`, `VENV_DIR`, `WEB_DIR`, and similar helper
exports are intentionally omitted here.

## Local Tooling

- `SOAC_PARENT_REPO=/path/to/parent/checkout`
  Optional override for `just setup-dev-env` inside a jj worktree. The recipe
  normally infers the parent checkout from a file-backed `.jj/repo`; the parent
  checkout owns `work/` as a regular artifact directory, `vendor/cpython`, and
  the shared offline state symlinked into the worktree: `.uv-cache`, `.uv/`,
  `.xdg/`, and `work/cargo-home`.

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
  [crates/soac_pyo3/src/jit_runtime.rs:96](/home/adam/project/soac-profile/crates/soac_pyo3/src/jit_runtime.rs#L96),
  eagerly compile lazy CLIF/JIT entries as they are registered instead
  of waiting for first execution.

- `SOAC_EXEC_TRACE=<selector>`
  In `SoacEnvConfig::from_env`, at
  [crates/soac_config/src/runtime.rs](/home/adam/project/soac-profile/crates/soac_config/src/runtime.rs),
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
  through `soac_jit_codegen`. Apply-mode indexed specialization hit/fallback
  summaries and deopt-entry summaries are emitted through
  `soac_specialization_runtime`. BlockPy module-cache hits and stores are emitted through
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
  `SOAC_PYTEST_EVENTS_LOG` when set, otherwise `work/logs/pytest_events.jsonl`.

- `SOAC_PYTEST_EVENTS_LOG=/path/to/events.jsonl`
  Overrides the pytest trace output path used when `SOAC_PYTEST_TRACE=1`.

- `SOAC_PYTEST_BATCH_TIMEOUT=<seconds>`
  Per-batch timeout for the parallel pytest runner used by `just pytest ...`,
  `just pytest-fast ...`, and `just test-all`. The default is `300` seconds.
  Set to `0` to disable the timeout.

- `SOAC_PYTEST_PROGRESS_INTERVAL=<seconds>`
  Interval for live "currently running pytest batch" reports from the parallel
  pytest runner. The default is `10` seconds. Set to `0` to disable live batch
  reports.

- `SOAC_RUN_SLOW_TESTS=1`
  Includes pytest tests marked `slow` in `just pytest ...` and `just test-all`.
  Slow tests are deselected by default so ordinary green runs skip intentionally
  expensive broad-mode coverage. When invoking pytest directly, pass
  `--run-slow` to include the same tests.

- `SOAC_CRANELIFT_OPT_LEVEL=none|speed|speed_and_size`
  Override the Cranelift optimization level used by the process JIT.
  Normal runtime and benchmark runs default to `speed_and_size`. The
  `just pytest`, `just test-all`, and `just run-cpython-tests` recipes default
  this to `none` unless the caller already set it, because correctness tests are
  latency-sensitive and should not spend cold-start time optimizing import-time
  helper code.

- `SOAC_JIT_COMPILE_WORKERS=<positive-integer>`
  Cap the number of worker threads used to compile functions inside one
  reserved process-JIT batch. Unset defaults to `min(available_parallelism, 4)`;
  set `1` to force single-worker compilation for timing comparisons.

- `SOAC_BACKGROUND_JIT=0`
  Disable import-time background JIT compilation. Background JIT is enabled by
  default for normal runtime use; pytest and CPython regression-test recipes
  default it off so correctness runs do not accumulate asynchronous compile
  work across many short-lived test modules.

- `SOAC_JIT_EMIT_REFCOUNTS=0`
  Disable generated JIT INCREF/DECREF emission by inlining the SOAC runtime
  refcount helpers as no-ops. Refcount emission is enabled by default; only
  `0`, `false`, `no`, or `off` disable it. This is an intentionally unsound
  performance experiment knob and leaks Python references. It also disables
  guard-miss deopt replay, because the replay interpreter depends on normal
  owned-reference bookkeeping from generated code.

- `SOAC_JIT_HANDLE_PENDING_CHECKS`
  Generated JIT loop-backedge calls to `_Py_HandlePending` are disabled by
  default. Set to `1`, `true`, `yes`, or `on` to enable them when CPython
  pending-call, signal-handling, thread-handoff, or async-exception latency
  matters for the workload.

## Counters And Specialization

- `SOAC_WORK_DIR=/path/to/work-dir`
  Runtime work directory for generated process-local output. In normal
  specialization workflows this directory contains:
  - `profile.bin`: specialization input recorded by the profile pass.
  - `verify.bin`: countered output recorded by the verify pass.
  - `events.jsonl`: default tracing JSONL when `SOAC_LOG` is not
    set.
  - `modules/`: root for cached pre-optimization BlockPy modules. Cached modules
    use stable per-module artifact paths such as
    `project/pkg/submod/mod.blockpy`, with source hash and build identity
    stored as cache metadata.

- `SOAC_OPT_MODE=none|profile|verify|apply`
  Select the runtime specialization phase:
  - `none`: run the ordinary unspecialized path, do not instrument
    specialization counters, do not read `$SOAC_WORK_DIR/profile.bin`,
    and do not write counter dumps. This is equivalent to leaving
    `SOAC_OPT_MODE` unset, but is useful when a parent environment may
    already set it.
  - `profile`: run unspecialized, instrument specialization input
    counters, and write `$SOAC_WORK_DIR/profile.bin`.
  - `verify`: read raw profile evidence from `$SOAC_WORK_DIR/profile.bin`,
    apply v3 decisions while building `InstrTyped`, instrument specialization
    input counters again, and write `$SOAC_WORK_DIR/verify.bin`. Verify mode
    exercises indexed store fast paths so their hit/fallback counters measure
    the specialized steady-state path.
  - `apply`: read raw profile evidence from `$SOAC_WORK_DIR/profile.bin`,
    apply v3 decisions while building `InstrTyped`, and emit no specialization
    counter dump files.
    When event logging is enabled through `SOAC_LOG` or the default
    `$SOAC_WORK_DIR/events.jsonl`, apply mode still records in-process
    indexed specialization hit/fallback and deopt-entry counts long enough to
    emit `soac_specialization_runtime` summary events at module teardown.
  Set `SOAC_WORK_DIR` for any mode that reads or writes counters. Leave
  `SOAC_OPT_MODE` unset, or set it to `none`, for the ordinary
  unspecialized/no-counter path.

- Runtime optimization uses the typed v3 path. `verify` and `apply` build the
  JIT module by lowering the cached pre-optimization BlockPy module to
  `TypedBlockPyModuleShape`, then applying v3 decisions from raw
  `profile.bin` evidence during typed JIT planning. Precompile uses the same
  raw profile evidence and cached pre-optimization BlockPy modules; there is no
  serialized optimization-plan artifact between profiling and codegen.

Notes:
- In normal workflows set one `SOAC_WORK_DIR` for the whole multi-pass
  run and change only `SOAC_OPT_MODE`.
- `SOAC_ENABLE_PROFILED_COLD_BLOCKS=1` replays `block_entry` counters
  from `$SOAC_WORK_DIR/profile.bin` as Cranelift `cold` block hints in
  `verify`/`apply`. This stays disabled by default; `profile` and
  `verify` only insert the underlying `block_entry` counters when this
  flag is enabled.
- The `apply` phase may emit explicitly marked `BEHAVIOR_CHANGE`
  fast paths. Today that includes raw indexed module-global / instance
  field stores outside module-init code, and undeclared known-builtin
  loads lowered to `RuntimeName` constants.

## Perf And Benchmarking

Benchmark sources live under the tracked `bench/` directory. Generated
benchmark results and other local artifacts live under the ignored `work/`
tree, with pystone benchmark runs writing to `work/bench/`.

- `just benchmark`
  The default benchmark recipe runs the transformed profile, verify,
  and specialized apply passes and writes the raw result directory under
  `work/bench/{change_id}_{commit_id}` for one-off runs or `work/bench/{change_id}`
  for finalized runs. It always records and prints the actual current `@`
  revision that it executed, so switch revisions first with `jj edit <rev>` if
  you want to benchmark some revision other than the current checkout. By
  default it keeps the benchmark log, raw counter files (`profile.bin`,
  `verify.bin`, `events.jsonl`), and the revision-scoped BlockPy module cache
  under `counters/modules`; it does not run `perf` and it does not build
  inspector-based counter/CLIF artifacts. The specialized apply phase reports
  both the default
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

- `just pyperformance [stock|soac|soac-single] [output] [benchmarks] [extra pyperformance run args...]`
  Run the pyperformance suite against the vendored CPython executable. The
  `stock` mode runs plain CPython. The default `soac` mode builds the release
  SOAC extension, runs pyperformance once with `SOAC_OPT_MODE=profile`, then
  runs it again with `SOAC_OPT_MODE=apply`; the requested `output` is the
  apply result, and the profile pyperf result is written beside it with a
  `.profile.json` suffix. Use `soac-single` for one-pass debugging; it honors
  the caller's `SOAC_OPT_MODE` and defaults to `none`.
  SOAC modes inject a recipe-local `sitecustomize` into pyperformance worker
  subprocesses and install `soac.import_hook` before benchmark imports. When
  `output` is omitted, final results are written to
  `work/pyperformance/{stock,soac}-<timestamp>.json`, and pyperformance's own
  benchmark virtual environments are created under `work/pyperformance/venv/`.
  When `benchmarks` is omitted, pyperformance uses its default suite selection;
  pass a comma-separated pyperformance benchmark list such as
  `json_dumps,richards` for a narrower run. The recipe defaults pyperf sampling
  to `--fast --min-time=0.05` so comparison runs collect multiple values without
  paying the full default pyperf runtime. Extra arguments are passed through to
  `pyperformance run`; `--rigorous` and `--debug-single-value` replace the
  default sample mode, and `--min-time=<seconds>` overrides the default
  calibration window. SOAC modes default `SOAC_MODULE_ENABLED` to the
  pyperformance benchmark source tree so the harness, pip, and pyperf internals
  stay on stock CPython unless the caller overrides the allow-list. They also
  default `SOAC_BACKGROUND_JIT=0`, because pyperformance uses short worker
  subprocesses where background compiler threads can outlive interpreter
  shutdown, and default `SOAC_COMPILE_MODE=eager` because lazy first-call
  compilation can block pyperformance's single worker loop. In SOAC modes, the
  recipe treats `SOAC_WORK_DIR` as a root and the worker wrapper writes each
  benchmark invocation's counters, logs, and module cache under a stable
  per-script-and-variant subdirectory so full-suite runs can profile many
  `__main__` scripts without source-hash or type-observation collisions.

- `just pyperformance-deep-profile-from-profile <result.json> <benchmark> [worker=<worker-dir>] [loops=<count>]`
  Replay one measured pyperformance worker directly from a prior SOAC
  profile/apply run and collect `perf` plus Speedscope artifacts for the worker
  body. The SOAC pyperformance wrapper records replay metadata in
  `<result>.soac-work/worker_manifest.jsonl`; this recipe selects a measured
  profile worker, rejects calibration workers, and asks for `worker=<worker-dir>`
  if the benchmark has more than one measured worker. Artifacts are written
  beside the selected worker by default under
  `<worker-dir>/worker_perf*`. The replay worker pauses through
  `SOAC_PYPERFORMANCE_MEASURE_READY_FILE` immediately before pyperf starts its
  measured values, so the attached profile excludes benchmark-module import and
  any pyperf warmups. Use this when pyperformance says a benchmark is slow and
  you need measured-worker attribution instead of profiling the pyperformance
  harness.

- `just precompile-shared-library counters=<profile.bin> out=<lib.so>`
  Offline precompile a counter-referenced set of cached BlockPy modules into
  relocatable object files and link them into a shared library. The counter
  file normally comes from a previous profile pass, and the matching
  pre-optimization BlockPy cache entries must still exist in the active
  `$SOAC_WORK_DIR/modules` cache. With the default benchmark cache isolation,
  that cache is the benchmark result's `counters/modules` directory. When
  `counters` is omitted, the recipe uses `$LAST_BENCHMARK_COUNTERS`. Set
  `SOAC_PRECOMPILED_LIBRARY` to the resulting `.so` to let runtime
  direct-function setup use matching precompiled entries.

- `SOAC_JIT_PERF_HELPER_FRAMES=1`
  In `fn should_preserve_perf_helper_frames`, at
  [crates/soac_jit/src/jit/specialized_helpers.rs:1700](/home/adam/project/soac-profile/crates/soac_jit/src/jit/specialized_helpers.rs#L1700),
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

- `PYPERFORMANCE_AFFINITY=<cpu-list>` / `PYPERFORMANCE_TIMEOUT=<seconds>`
  Optional `just pyperformance` pass-throughs to pyperformance's `--affinity`
  and `--timeout` options. If `PYPERFORMANCE_AFFINITY` is unset, the recipe uses
  `BENCHMARK_CPU` as the affinity list when that existing benchmark knob is set.

- `PYPERFORMANCE_INHERIT_ENV_EXTRA=NAME[,NAME...]`
  Adds environment variables to the `--inherit-environ` list used by
  `just pyperformance mode=soac`. The recipe already inherits the SOAC runtime
  variables needed by the transformed benchmark workers.

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
