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
  Pipeline orchestration around lowering and codegen module caching. It exposes
  production and test lowering entrypoints and owns the cache path/metadata
  helpers used by the runtime and optimizer.

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
  Python syntax is turned into progressively more resolved compiler IR.

- `soac-macros`
  Procedural macros used by the lowering and IR layers to reduce repetitive
  enum delegation and match boilerplate. It should stay mechanical and avoid
  owning compiler semantics.

- `soac-opt`
  Optimization planning and artifact generation from profile evidence and
  cached BlockPy modules. It owns legacy and v3 plan formats, specialization
  decisions, emitted plan sidecars, and optimization-family status reporting.

- `soac-pyo3`
  The `_soac_ext` Python extension module. It bridges CPython import-hook
  callbacks into SOAC lowering, optimization-plan loading, JIT module creation,
  and runtime execution.

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

- `soac-clif-snippet`
  Profiles a small Python snippet, renders pre-inlining and final specialized
  CLIF, and prepares annotation context for explaining generated blocks,
  guards, counters, and helper calls.

- `soac-profile-benchmark`
  Runs the SOAC pystone profile/verify/apply benchmark workflow and summarizes
  the generated `work/bench/` result directory.

- `summarize-cpython-failures`
  Summarizes CPython regrtest logs, computes file and test-case totals, and
  groups failures by likely root cause.

## CLI Inspection Tools

Most command-line inspection tools live in `soac-inspector` and can be run as
`cargo run -p soac-inspector --bin <tool> -- ...`.

- `soac-inspector`
  Starts the local web inspector server. It serves the interactive pass,
  BlockPy, CLIF, and typed-instruction views, binding to `HOST`/`PORT` or
  `127.0.0.1:8000` by default.

- `diet-python`
  Lowers a Python file through the transform pipeline and prints the final
  rewritten Python source. Pass `--timing` to emit per-pass timing JSON on
  stderr.

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

- `decide_optimizations`
  Reads profile counters plus cached BlockPy modules and writes legacy `mod.opt`
  or v3 `mod.optv3` optimization plans under a module-cache root.

- `print_optimization_plan`
  Pretty-prints a legacy `mod.opt` optimization plan.

- `print_optimization_plan_v3`
  Pretty-prints a v3 `mod.optv3` optimization artifact, with `--details` for
  region and emission detail.

- `precompile_blockpy`
  Uses profile counters and cached BlockPy modules to compile referenced
  modules into object files and link an offline shared library for
  `SOAC_PRECOMPILED_LIBRARY`.

- `annotate_cranelift_perf`
  Correlates perf samples with SOAC JIT basic-block maps for a benchmark result
  directory, writes annotated VCode files, and prints block-level sample rows.

- `regen_snapshots`
  Regenerates ignored `snapshot/` fixtures and summary rows from checked-in
  snapshot source cases. This is mainly a maintenance tool for inspector and
  lowering snapshot workflows.

# Development Environment

Install the Python-side venv and the nightly Rust codegen backend used by
`soac_jit`:

```
just setup-dev-env
```

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

# Unsupported Source Forms

- SOAC does not support string literals containing lone surrogate escapes such
  as `"\uD800"`. These are uncommon, cannot be represented as ordinary Rust
  `str` data after parsing, and should fail explicitly instead of being routed
  through a runtime `eval()` workaround.

# Environment Variables

This repo consults a number of environment variables directly. The list
below is the user-facing set that changes runtime behavior, profiling,
benchmarking, test wrappers, or the local web UI. Pure `Justfile`
plumbing such as `REPO_ROOT`, `VENV_DIR`, `WEB_DIR`, and similar helper
exports are intentionally omitted here.

## Local Tooling

- `UV_CACHE_DIR`, `UV_TOOL_DIR`, `UV_TOOL_BIN_DIR`, `XDG_CACHE_HOME`,
  `XDG_DATA_HOME`, `XDG_RUNTIME_DIR`, and `CARGO_HOME`
  The `.envrc` and `Justfile` point these at repo-local directories by default
  so uv package cache, installed tools, and XDG state stay under the working
  tree. `XDG_RUNTIME_DIR` defaults under `work/tmp/`, and `CARGO_HOME` defaults
  to `work/cargo-home`. The
  `Justfile` also respects pre-set values for these variables, which allows
  temporary worktrees to use explicit writable shared cache roots.
  `just setup-dev-env` installs `ruff` into the repo-local uv tool bin
  directory.

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
  In `fn parse_trace_env`, at
  [crates/soac_lowering/src/passes/trace/mod.rs:20](/home/adam/project/soac-profile/crates/soac_lowering/src/passes/trace/mod.rs#L20),
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
  Normal runtime and benchmark runs default to `speed`. The
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
  default for normal runtime use; pytest recipes default it off so correctness
  runs do not accumulate asynchronous compile work across many short-lived test
  modules.

- `SOAC_JIT_EMIT_REFCOUNTS=0`
  Disable generated JIT INCREF/DECREF emission by inlining the SOAC runtime
  refcount helpers as no-ops. Refcount emission is enabled by default; only
  `0`, `false`, `no`, or `off` disable it. This is an intentionally unsound
  performance experiment knob and leaks Python references. It also disables
  guard-miss deopt replay, because the replay interpreter depends on normal
  owned-reference bookkeeping from generated code.

## Counters And Specialization

- `SOAC_WORK_DIR=/path/to/work-dir`
  Runtime work directory for generated process-local output. In normal
  specialization workflows this directory contains:
  - `profile.bin`: specialization input recorded by the profile pass.
  - `verify.bin`: countered output recorded by the verify pass.
  - `events.jsonl`: default tracing JSONL when `SOAC_LOG` is not
    set.
  - `modules/`: root for cached pre-optimization BlockPy modules and sibling
    `mod.opt` / `mod.optv3` optimization plans. Cached modules use stable
    per-module artifact paths such as `project/pkg/submod/mod.blockpy`, with
    source hash and build identity stored as cache metadata.

- `SOAC_OPT_MODE=none|profile|verify|apply`
  Select the runtime specialization phase:
  - `none`: run the ordinary unspecialized path, do not instrument
    specialization counters, do not read `$SOAC_WORK_DIR/profile.bin`,
    and do not write counter dumps. This is equivalent to leaving
    `SOAC_OPT_MODE` unset, but is useful when a parent environment may
    already set it.
  - `profile`: run unspecialized, instrument specialization input
    counters, and write `$SOAC_WORK_DIR/profile.bin`.
  - `verify`: read per-module optimization plans from the active module cache,
    selected by `SOAC_OPT_PLAN_MODE`, apply their specializations,
    instrument specialization input counters again, and write
    `$SOAC_WORK_DIR/verify.bin`. Verify mode exercises indexed store
    fast paths so their hit/fallback counters measure the specialized
    steady-state path.
  - `apply`: read per-module optimization plans from the active module cache,
    selected by `SOAC_OPT_PLAN_MODE`, apply their specializations,
    and emit no specialization counter dump files.
    When event logging is enabled through `SOAC_LOG` or the default
    `$SOAC_WORK_DIR/events.jsonl`, apply mode still records in-process
    indexed specialization hit/fallback and deopt-entry counts long enough to
    emit `soac_specialization_runtime` summary events at module teardown.
  Set `SOAC_WORK_DIR` for any mode that reads or writes counters. Leave
  `SOAC_OPT_MODE` unset, or set it to `none`, for the ordinary
  unspecialized/no-counter path.

- `SOAC_OPT_PLAN_MODE=auto|legacy|v3`
  Select which serialized optimization-plan artifacts `verify` and `apply`
  consume. The default `v3` mode requires `mod.optv3` and errors instead of
  falling back to a legacy plan. `auto` prefers `mod.optv3` and falls back to
  legacy `mod.opt`. `legacy` ignores `mod.optv3`.

- `SOAC_DECIDE_OPT_MODE=legacy|v3`
  Select which artifact family the Justfile profile-to-plan recipes generate.
  The default is `v3`, so `just benchmark`, `just benchmark-verify`, and
  `just precompile-shared-library` write `mod.optv3` from cached unoptimized
  BlockPy modules and raw profile evidence. Set `legacy` only when comparing the
  old planner path.

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
  `verify.bin`, `events.jsonl`), generated optimization plans, and the
  revision-scoped BlockPy module cache under `counters/modules`; it does not
  run `perf` and it does not build inspector-based counter/CLIF artifacts. The
  profile pass is immediately followed by `decide_optimizations --mode v3` by
  default, so verify and apply consume serialized v3 plan artifacts rather than
  raw counters or legacy decisions. Set `SOAC_DECIDE_OPT_MODE=legacy` to compare
  the old planner path. The specialized apply phase reports both the default
  refcounts-enabled throughput and an additional unsound
  `SOAC_JIT_EMIT_REFCOUNTS=0` diagnostic throughput.

- `just benchmark-deep-profile`
  Run `just benchmark`, then add the heavier follow-on artifacts in the
  same result directory: counter/specialization text dumps, rendered
  specialized CLIF/VCode/CFG, `perf` capture, and perf-annotated VCode.

- `just benchmark-deep-profile-from-profile <result-dir>`
  Start from an existing result directory with `counters/profile.bin`,
  regenerate optimization plans, rerun only the verify pass to produce
  `verify.bin`, then add the same deep-profile artifacts without rerunning the
  profile pass.

- `just precompile-shared-library counters=<profile.bin> out=<lib.so>`
  Offline precompile a counter-referenced set of cached BlockPy modules into
  relocatable object files and link them into a shared library. The recipe
  regenerates optimization plans from the counter file before compiling. The
  precompile JIT path follows `SOAC_OPT_PLAN_MODE`: the default `v3` mode
  requires `mod.optv3`, while `auto` prefers `mod.optv3` and falls back to
  legacy `mod.opt`. The
  counter file normally comes from a previous profile pass, and the matching
  pre-optimization BlockPy cache entries must still exist in the active
  `$SOAC_WORK_DIR/modules` cache. With the default benchmark cache isolation,
  that cache is the benchmark result's `counters/modules` directory. When
  `counters` is omitted, the recipe uses `$LAST_BENCHMARK_COUNTERS`. Set
  `SOAC_PRECOMPILED_LIBRARY` to the resulting
  `.so` to let runtime direct-function setup use matching precompiled entries.

- `cargo run -p soac_inspector --bin decide_optimizations -- --counters <profile.bin> --out <modules-root>`
  Load a counter dump once, scan the cached BlockPy module root for
  `mod.blockpy` files, and write sibling binary optimization-decision artifacts
  using stable module artifact paths such as `python-stdlib/typing/mod.opt`.
  Pass `--mode v3` to write `mod.optv3` artifacts from raw profile evidence and
  cached unoptimized BlockPy modules instead of writing legacy `mod.opt` plans.
  Pass `--module-root <root-dir>` to scan a different input root, or one or more
  `--module <mod.blockpy>` arguments for narrower debugging.
  Use `cargo run -p soac_inspector --bin print_optimization_plan -- --plan <mod.opt>`
  to pretty-print a legacy plan for inspection, or
  `cargo run -p soac_inspector --bin print_optimization_plan_v3 -- --plan <mod.optv3>`
  to inspect a v3 artifact summary. `just benchmark` runs v3 mode after the
  profile pass by default. In `SOAC_OPT_MODE=verify|apply`,
  `SOAC_OPT_PLAN_MODE` controls runtime plan selection: the default `v3`
  requires a matching serialized v3 artifact, while `auto` prefers a matching
  `mod.optv3` in the active module cache and falls back to legacy `mod.opt`.

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
  In [`fn main`, at [crates/soac_inspector/src/main.rs:8](/home/adam/project/soac-profile/crates/soac_inspector/src/main.rs#L8)],
  control the bind address for the local inspector server. The `Justfile`
  default is `127.0.0.1`.

- `PORT=<port>`
  In [`fn main`, at [crates/soac_inspector/src/main.rs:9](/home/adam/project/soac-profile/crates/soac_inspector/src/main.rs#L9)],
  control the bind port for the local inspector server. The `Justfile`
  default is `8000`.
