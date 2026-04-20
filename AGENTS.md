# AGENTS

## WHY AND HOW?

`soac` is a just-in-time compiler for Python. The context here matters
because it should affect engineering decisions:

* The default correctness bar is CPython user-visible behavior.
  If `soac` intentionally diverges, that divergence should be explicit
  and justified, not introduced accidentally as part of an optimization
  or refactor.

* Optimization priorities should favor long-running or batch workloads
  over changes that only improve cold-start behavior.

* Prefer an explicit end-to-end pipeline from parsing through lowering
  and code generation, rather than hiding behavior in ad hoc runtime
  patches.

* Instrumentation and observability are aligned with the intended
  architecture. Changes that make optimization feedback easier to
  collect and reason about are usually a good fit.


## DESIGN GOALS


1. SOAC should *always* either have the same user-visible behavior as
   CPython, or (in uncommon cases) fail explicitly rather than
   producing incorrect results.  This includes behavior around
   evaluation order, when refcounting / when objects are freed, and
   interaction with C extensions.

2. Keep the codebase conceptually small.  The ideal is that every
   concept is represented by one type, and in one part of the
   codebase.  So for example, we keep raw variable names as a bare
   String until the name_binding pass, which then encapsulates all of
   the translation to physical storage locations.

3. As an extension to #2, do not keep abstractions or codepaths around
   purely for "backwards compatability".  If something is depended on
   only by tests, either update the tests to the production path or
   delete them.

4. Prefer representing resolved compiler behavior in `BlockPyModule`,
   `Instr`, or a sidecar validated against them rather than hiding it in
   codegen. By the time fully resolved functions reach codegen, codegen
   should be dumb and mechanical: it should emit the selected operation
   shape, not rediscover semantic decisions that could have been
   validated earlier.

5. Keep payload nodes context-free.  Nested IR payloads like names and
   literals should represent only the payload itself; operation kind
   and source metadata belong on the enclosing IR node such as
   `Load`/`Store`/`Del` or `LiteralValue`, not duplicated inside the
   payload.

6. Lower source names in stages. A raw AST `ExprName` may only lower
   directly to `UnresolvedName`. `LocatedName` must only come from an
   explicit name-binding or other explicit location decision, never
   from a blind default location.

7. Avoid global mutable state; if needed there should be a single
   global structure and then all consumers take that structure rather
   than directly accessing the global.

8. Keep SOAC metadata explicit across CPython callback boundaries.
   Do not smuggle compiler/runtime state through temporary Python
   attributes or thread-local context just to reach a fixed-signature
   CPython hook; prefer an explicit post-create/post-callback init step
   or a clearly-owned Rust state object.

9. Keep `soac-runtime` visibly raw and ABI-shaped. It is the very-hot
   local runtime layer that gets inlined into generated code: avoid PyO3
   wrapper types there, name hand-written CPython layout mirrors with a
   `RawPy*` prefix, and keep casts clustered at ABI boundaries so direct
   layout access remains obvious in review.

10. Treat generator and coroutine closure bindings like ordinary closure
   bindings. If a cleanup, ownership, or specialization path is unsound for
   generator runtime cells, make the ownership state explicit or use a more
   precise key; do not add broad generator/module exclusions that hide the
   binding model.

## THE DEVELOPMENT LOOP

When I submit a request, if it's a simple, fully-specific or
mechanical request to change something, just go ahead and do it.
Otherwise make a plan for that change.

### Planning

The plan should include:

 * A description of the individual steps to accomplish the goal
 * Where appropriate, short code samples to illustrate what will be done.
 * Make note of any particularly challenging parts of the change.

If the request is somehow unclear, or there are multiple options for
how to solve it, ask for clarification.

Once the plan is approved, follow "Making changes" for each individual step.

### Making changes

1. Start from a synchronized workspace.

If `jj status` reports a stale workspace, run `jj workspace
update-stale` before editing. Active work should stay in the workspace's
current `@`. Do not treat another workspace's live `@` as a dependency.

2. Update the working commit ("@") with a descriptive title and, when
   applicable, a body that includes the request and the plan.

3. Keep debugging and repros repo-native.
   Prefer `Justfile` entrypoints over raw interpreter commands.
   Use `just py ...`, `just pytest ...`, and `just test-all` for transformed-runtime work unless you are intentionally debugging raw vendored CPython behavior.
   For isolated transformed-runtime repros, prefer `tests._integration.transformed_module(...)`.

4. Keep generated artifacts out of logical changes unless explicitly requested.
   The generated `snapshot/` directory is ignored and should not be committed.

5. Add focused regression coverage for real bugs.
   For each CPython regression you fix, add a minimal reproducing integration
   test under `tests/` first.
   If you discover a performance regression because a specialization stopped
   applying, add a focused specialization regression test for that case. These
   tests should prove the optimization decision, plan, or structured codegen
   shape that matters, not assert on rendered CLIF/BlockPy text.
   If diagnosing a hang, use `just capture-test-stacks <pid>` on the apparently
   stuck test process or the `just test-all` child PID printed in progress
   output, add follow-up instrumentation where practical, and leave behind a
   focused regression or assertion for that hang shape.
   Avoid render-only tests for BlockPy/CLIF/inspector text. Prefer behavior,
   structure, or API tests; exact renderer output changes too often to be a
   useful default regression surface.

6. Avoid render-as-behavior tests.

   Outside tests for the renderer/pretty-printer itself, avoid testing a
   compiler behavior by rendering an AST/IR/debug value to text and asserting
   that an expected string appears. These tests are low signal and create
   fixup churn on unrelated render changes. Prefer assertions on the actual
   AST, IR, CFG edge, binding/storage layout, module constant, or other
   structured output shape.

7. Keep specialization docs in sync.

   If you add or materially change a specialization, update
   `docs/SPECIALIZATION.md` in the same logical change. The doc should
   describe what profiling input is recorded, what codegen shape is
   emitted, and the current limitations, soundness boundaries, or likely
   extensions.

8. Keep specialization regression tests structured and easy to write.

   Prefer a dedicated specialization-regression test family over broad
   benchmark assertions. A good test should:
   - build or load the smallest Python snippet that exercises the missed
     specialization;
   - provide the minimal counter/profile/optimization-plan evidence needed to
     make the specialization eligible;
   - run the same decision/apply path used by production, or the closest
     structured helper for that specialization;
   - assert on typed outputs such as `OptimizationDecision`,
     `PlannedReplacement`, selected direct-call target, guard shape,
     specialized helper choice, structured CLIF IR facts, or emitted function
     metadata;
   - avoid throughput thresholds, exact rendered strings, and incidental block
     labels unless the test is explicitly for the renderer.

   When adding the first test in a new specialization area, add a small fixture
   helper that names the intent in the test body, for example
   `assert_direct_call_decision(...)`, `assert_inline_decision(...)`, or
   `assert_exact_type_guard(...)`. The purpose should be obvious from the test
   name: "this specialization still applies for this source/profile shape."

9. Keep environment-variable docs in sync.

   If you add or materially change an environment variable that controls
   runtime behavior, testing, benchmarking, profiling, or the local web
   tooling, document it in `README.md` and add or update the relevant
   note in `AGENTS.md` in the same logical change.

10. Keep runtime helper inventory in sync.

   If you add, remove, rename, or move runtime helper functions in
   `soac-runtime`, `soac-jit/src/jit/specialized_helpers.rs`, or
   `soac_py/src/soac/runtime.py`, update `docs/RUNTIME_FUNCTIONS.md`
   in the same logical change.

11. Record finalized performance changes.

When a change is expected to affect performance, validate it with a before/after
benchmark before treating it as complete. Use the repo benchmark comparison
workflow unless there is a specific reason to use a narrower measurement, and
report both headline throughput and relative delta.

When a performance change is complete enough that you intend to keep it,
rebase the finished change onto `main`, run `$soac-profile-benchmark`, write the
finalized benchmark result to `work/bench/{change_id}`, and
append an entry to `docs/CODEX_OPT_LOG.md` in the same logical change.
Use `work/bench/{change_id}_{commit_id}` only for one-off test benchmarks while
iterating. Keep log entries succinct: include the jj change id, a short summary
of the optimization, the benchmarked throughput delta, and the before/after
headline numbers. Do not paste validation checklists, full command lines, or
long run logs.

The benchmark recipes record and print the actual current `@` revision; they do
not take a revision argument. To benchmark another revision, switch the
workspace to that revision first, for example with `jj edit <rev>`. Use
`jj new <rev>` only when you intentionally want a fresh child revision and
benchmark artifacts named for that child rather than the existing revision.

12. Run the full gate before submitting code changes.

Run `just test-all` before submitting unless the change is docs-only,
such as `todo/TODO.md`, `AGENTS.md`, or similar documentation-only files.
For fast feedback on Rust changes that may affect crate test targets,
run `cargo check -p soac-jit --tests` before the full gate; it
type-checks the `soac-jit` crate including tests without running the
entire transformed-runtime suite.
Put test output in `work/logs/`. Summarize the failures, separate expected
failures from unexpected failures, investigate the root cause, report
it, then fix it.

13. When a logical set of changes is complete, freeze it before
   integrating it.

Run `jj new` so the finished work is no longer the live working commit.
Rebase and integrate the finished change, not the live `@`.

14. Try to advance `main` directly to the finished head.

Prefer `jj bookmark move main -t <finished-head>` when the finished
change is already a descendant of the current `main`. This avoids
unnecessary rebases and duplicate sibling revisions.

15. If advancing `main` fails because the finished head is not a
   descendant of `main`, rebase the finished commit or finished stack
   onto `main`.

Use `jj rebase` on the finished revision or stack root so the completed
work sits directly on top of the current shared base.

16. Resolve any conflicts and rerun the relevant tests.

The rebased change is not ready to advance `main` until conflicts are
resolved and the relevant checks have been rerun.

If conflict recovery involved restoring files from a known-good pre-rebase
revision, inspect `jj diff --summary <good-rev>..main` and restore the entire
logical file set before validating. Partial restores can leave the change in an
internally inconsistent state that compiles or fails for misleading reasons.

17. Advance `main` to the finished head.

This is the synchronization point. Once `main` moves, the finished work
becomes the new shared base for future work.

If the user asks to rebase, merge, or otherwise put a completed stack "on
main", do not stop after the stack is merely based on `main`. After conflict
resolution and validation, explicitly move `main` to the validated head and
verify the bookmark in `jj log`.

18. When another agent advances `main`, refresh and continue on top of
    it.

Run `jj workspace update-stale` and rebase your live work onto the new
`main` as needed. Other agents should only depend on `main`, not on a
peer workspace's live `@`.

19. Report the result: run `jj diff --stat` on the completed change and
report its output. At the end of each completed turn or substantial pass,
summarize what is currently being worked on, the state it is in, and the
next concrete steps for that work. If I did not ask to approve each step
after the plan, continue with the next step.

The goal of the `jj` workflow is to keep `main` as the clear shared
synchronization point without letting one codex instance rewrite another
instance's live working commit.

### Continuous workflow improvement

Treat every task as a chance to improve the project workflow, not just the
immediate code. At the end of each turn or substantial pass, do a brief
internal retro:

- Did I hit an avoidable environment, tooling, sandbox, dependency, cache,
  workspace, test, benchmark, or missing-artifact problem?
- Did I use an awkward manual sequence that should be a Justfile recipe,
  script, preflight check, or documented convention?
- Did I make an assumption that caused rework, or did the user correct a
  misunderstanding that should become a reusable rule?
- Did a test failure, xfail, log gap, panic message, or benchmark artifact make
  diagnosis harder than it should have been?
- Did I notice project behavior that is surprising, underdocumented, or likely
  to trip up the next agent?

If the answer is yes and the fix is concrete, report it under
`Workflow improvement` or `Project feedback`. Include what happened, why it
cost time or risked correctness, the specific proposed change, and where it
belongs: `AGENTS.md`, `README.md`, `Justfile`, a script, a test, code
instrumentation, or user workflow. If the user request is missing a detail that
predictably affects validation, runtime mode, workspace ownership, benchmark
interpretation, or conflict resolution, surface that early. When the likely
default is clear, proceed with that default and state it. When the choice
changes the meaning of the result, ask before doing expensive work.

Do not silently absorb recurring setup failures, missing-extension errors,
sandbox denials, stale-workspace problems, accidental tool misuse, file-lock
waits, perf sample loss, cache misses, or large unexpected artifact generation.
Treat "no parallel Cargo without separate target dirs" as a hard rule: do not
run multiple `cargo` commands in parallel unless each uses an explicitly
separate target directory, because shared artifact/package locks add noise and
turn focused validation into avoidable waiting. Likewise, if `just py` or
`just pytest` repeatedly pays the full unchanged-environment venv/native build
setup cost, report that as workflow debt and prefer or propose a lighter fast
path for focused checks instead of silently absorbing the rebuild tax.
Prefer durable project-native fixes over reminders: Justfile preflights,
clearer recipes, deterministic cache/output directories, better
panic/source-location messages, smaller repro tests, log summaries,
environment-variable docs, and removing obsolete or misleading tests. If there
is no actionable improvement, say nothing about it.

When reporting a completed pass, include a short `Environment notes` line if
anything non-code affected the run. If there were no such issues, say
`Environment notes: none`. Also include the current work thread and next steps
so a later turn can resume without rediscovering context.


## APPENDIX

### Testing and runtime entrypoints

- `just test-all`
  Full gate for non-doc changes.
- `just pytest ...`
  Authoritative transformed-runtime pytest entrypoint.
- `just pytest-fast ...`
  Fast transformed-runtime pytest entrypoint for repeated focused checks. It
  reuses the existing venv and debug extension when `vendor/cpython/python`,
  `soac_py/pyproject.toml`, `uv.lock`, and workspace Rust inputs are unchanged,
  and falls back to the full build path when they are stale or missing. The
  root pytest config defaults collection to `tests/`, so option-only invocations
  such as `just pytest-fast -q` do not recurse into vendored CPython tests.
- `just capture-test-stacks <pid> [out]`
  Hang diagnostic for test runs. Pass the PID printed by `just test-all` for
  `cargo-test`/`pytest`, or the PID of a stuck `just`, `pytest`, or
  `cargo test` process. The recipe walks the process tree and writes native
  gdb stacks plus Python `py-bt` stacks, when available, to `work/logs/` by default.
  It may need ptrace permission/CAP_SYS_PTRACE depending on host
  `/proc/sys/kernel/yama/ptrace_scope`.
- `just py ...`
  Best entrypoint for ad hoc transformed-runtime repros outside pytest.
- `just py-fast ...`
  Fast transformed-runtime Python entrypoint for tight edit/repro loops. It
  uses the same unchanged-environment reuse rules as `just pytest-fast ...`.
- `just run-cpython-tests ...`
  Use for vendored CPython regrtest runs.
- `$soac-profile-benchmark`
  Default skill for performance benchmark requests. This uses the
  default pystone workflow: profile, verify, and specialized apply benchmark.
  Tracked benchmark sources live under `bench/`; generated benchmark result
  artifacts live under ignored `work/bench/`.
  One-off test benchmarks write `work/bench/{change_id}_{commit_id}`. Finalized
  benchmarks for changes that are being merged to `main` must run after rebasing
  onto `main` and write `work/bench/{change_id}`. Use `just benchmark-deep-profile`
  when the user explicitly wants inspector/CLIF artifacts or perf capture, and
  use `just benchmark-deep-profile-from-profile <result-dir>` to extend an
  existing `counters/profile.bin` result without rerunning the profile pass.
`just benchmark` records the actual current `@` revision in the result header
and does not accept a revision argument; switch revisions first with `jj edit`
or, if you intentionally want a fresh child revision, `jj new`. Benchmark
recipes keep the BlockPy module cache under the current result's
`counters/modules` directory so in-place before/after revision runs cannot reuse
another revision's cache.
  Report the specialized apply-pass median from the result directory's
  `benchmark.txt` unless I explicitly ask for the warm unspecialized baseline.
  The benchmark also attempts an unsound `SOAC_JIT_EMIT_REFCOUNTS=0` diagnostic
  apply pass after the default refcounts-enabled pass. Keep the default
  refcounts-enabled median as the headline; if the diagnostic fails, report the
  failure without treating it as a failed production benchmark.
- `soac.ipython`
  IPython extension for interactive optimization inspection. Launch it with
  `just ipython`, load it with `%load_ext soac.ipython`, profile a top-level
  recoverable Python function with `%soac-profile func(args...)`, then print
  specialized CLIF with `%soac-clif func`, Codex-annotated CLIF with
  `%soac-clif-annotate func`, or lowered assembly-like VCode with
  `%soac-vcode func`. It materializes the function source into a temporary
  transformed module, uses `soac-inspector` for rendering, and uses
  `codex exec` for annotation, so it is for local exploration rather than
  benchmark-quality measurement.
- Repo-local uv state
  `.envrc` and `Justfile` keep uv, XDG, and cargo state under the repo with
  `UV_CACHE_DIR`, `UV_TOOL_DIR`, `UV_TOOL_BIN_DIR`, `XDG_CACHE_HOME`,
  `XDG_DATA_HOME`, `XDG_RUNTIME_DIR`, and `CARGO_HOME`. `XDG_RUNTIME_DIR` and
  `CARGO_HOME` default under `work/tmp/`. Module artifacts are rooted under the
  active `$SOAC_WORK_DIR/modules` directory. The `Justfile` respects pre-set
  values for those repo-local state variables. `just setup-dev-env` reuses an
  already-installed nightly Rust toolchain and Cranelift codegen component
  rather than upgrading them on every run, because nightly refreshes force
  rebuilds. It also installs the repo-local `ruff` command. Test and benchmark
  recipes use `UV_OFFLINE=1` for uv-backed venv refreshes; use
  `just update-venv` or rerun `just setup-dev-env` when dependency changes
  intentionally require network access.
- `SOAC_PARENT_REPO`
  Optional override for `just setup-dev-env` in a jj worktree. The setup recipe
  normally infers the parent checkout from a file-backed `.jj/repo`; the parent
  owns shared offline state and `work/` as a regular artifact directory. The setup
  recipe symlinks `vendor/cpython`, `work/`, `.uv-cache`, `.uv/`, `.xdg/`, and
  `work/tmp/cargo-home` from that parent into the worktree, and errors instead of
  creating isolated empty offline caches when neither
  inference nor the override can identify the parent.
  When sandboxing would otherwise block shared benchmark writes or jj metadata
  updates, run Codex with the worktree and parent checkout as writable roots.
  Prefer an absolute parent path such as `--add-dir /home/adamh/code/soac`;
  relative paths in project config can be resolved relative to the config file
  rather than the shell cwd, and a mistaken path can expose a directory inside
  the worktree instead of the shared parent repo.
- SOAC Rust runtime/JIT environment variables are parsed into a typed config
  once at the relevant entrypoint. Unset variables use documented defaults;
  present typed variables must use recognized values. Boolean knobs accept `1`,
  `true`, `yes`, or `on` for true and `0`, `false`, `no`, or `off` for false.
- `SOAC_WORK_DIR` / `SOAC_OPT_MODE`
  Normal specialization runs use one work directory with conventional
files: `profile.bin` for specialization input, `verify.bin` for the
countered verification pass, and `events.jsonl` for default JSON
tracing output. Cached pre-optimization BlockPy modules and sibling `mod.opt`
plans live under `$SOAC_WORK_DIR/modules`. Set `SOAC_OPT_MODE=none`, `profile`,
`verify`, or `apply`; recipes should pass the same `SOAC_WORK_DIR` and change only
the mode between passes. `none` is the explicit ordinary
  unspecialized/no-counter mode and should not read or write counter
  dumps. `profile` writes raw evidence to `profile.bin`; run
  `decide_optimizations` to turn that evidence plus cached BlockPy modules into
  per-module `mod.opt` plans before entering `verify` or `apply`. `verify`
  exercises indexed store fast paths so hit/fallback counters measure the
  specialized steady-state path. `apply` still skips counter dump files, but
  when event logging is enabled it records indexed specialization hit/fallback
  counts and deopt-entry counts long enough to emit
  `soac_specialization_runtime` summary events.
- `SOAC_ENABLE_PROFILED_COLD_BLOCKS`
  Optional opt-in for replaying `block_entry` counters from
  `$SOAC_WORK_DIR/profile.bin` as Cranelift `cold` block hints during
  `verify`/`apply`. This is disabled by default; the counters remain
  recorded in `profile`/`verify` either way.
- `SOAC_VALIDATE_OPT_V3`
  Optional opt-in for validating the current optimizer-v3 exact-int branch
  pipeline while loading `mod.opt` plans in `verify`/`apply`. This runs v3
  extraction, planning, plan validation, and mechanical emission against the
  live lowered function/evidence and passes the resulting artifact to JIT
  codegen inputs. When the artifact contains the represented exact-int
  add/compare branch, JIT term lowering consumes the mechanical v3 region and
  its local generic fallback; other shapes remain on the existing lowering path.
- `BENCHMARK_CPU` / `BENCHMARK_CONSTANT_CLOCKS`
  The benchmark recipes use
  [scripts/run_benchmark_with_cpu_mode.sh](/home/adam/project/soac-profile/scripts/run_benchmark_with_cpu_mode.sh)
  for optional CPU pinning and optional constant-clock mode. By default
  benchmarks are not CPU-pinned and do not change `cpufreq`; set
  `BENCHMARK_CPU=<cpu>` to pin with `taskset`, and set
  `BENCHMARK_CONSTANT_CLOCKS=1` only when you explicitly want the wrapper
  to request steadier clocks. If you add or change benchmark-stability
  knobs, document them in `README.md` and in this appendix note.
- `PERF_CALL_GRAPH`
  The perf profiling recipes default to `PERF_CALL_GRAPH=dwarf,65528`
  rather than a shallower stack dump, because the larger DWARF capture
  materially reduces truncated mixed JIT/CPython stacks in the exported
  profiles. If you change that default or add related perf-stack knobs,
  document them in `README.md` and in this appendix note.
- `SOAC_LOG`
  Controls SOAC Rust diagnostics through `tracing-subscriber` filter
  syntax. Use focused targets such as `SOAC_LOG=soac_jit=info` or
  `SOAC_LOG=soac_blockpy=trace`; append
  `;json=/path/to/events.jsonl` to write tracing JSONL there instead
  of formatted stderr. Module-load timing is emitted by the
  `soac_module_load` target; JIT-codegen timing is emitted by
  `soac_jit_codegen`; apply-mode indexed specialization hit/fallback
  summaries are emitted by `soac_specialization_runtime`; BlockPy module-cache
  hits and stores are emitted by `soac_blockpy_module_cache`. When `SOAC_LOG` is
  unset and `SOAC_WORK_DIR` is set, the default event log is
  `$SOAC_WORK_DIR/events.jsonl` and includes `soac_specialization_runtime`.
- `SOAC_PYTEST_TRACE` / `SOAC_PYTEST_EVENTS_LOG`
  `just pytest ...` and `just test-all` do not write the verbose JSON module
  load/JIT-codegen trace by default. Set `SOAC_PYTEST_TRACE=1` to enable it
  when `SOAC_LOG` is unset. `SOAC_PYTEST_EVENTS_LOG` overrides the output path,
  which defaults to `work/logs/pytest_events.jsonl`.
- `SOAC_PYTEST_BATCH_TIMEOUT` / `SOAC_PYTEST_PROGRESS_INTERVAL`
  The parallel pytest runner used by `just pytest ...`, `just pytest-fast ...`,
  and `just test-all` reports currently running batches every
  `SOAC_PYTEST_PROGRESS_INTERVAL` seconds, default `10`, and kills any one
  batch that exceeds `SOAC_PYTEST_BATCH_TIMEOUT` seconds, default `300`. Set
  either value to `0` to disable that behavior. Use these live batch labels
  before falling back to host-process inspection when sandbox process namespaces
  hide the stuck pytest child.
- `SOAC_RUN_SLOW_TESTS`
  `just pytest ...` and `just test-all` deselect pytest tests marked `slow` by
  default. Set `SOAC_RUN_SLOW_TESTS=1` to include intentionally expensive tests
  such as broad import-hook coverage. Direct pytest invocations can also pass
  `--run-slow`.
- `SOAC_PRECOMPILED_LIBRARY`
  Optional path to an offline-precompiled SOAC shared library. When set, runtime
  direct-function compilation first tries to load matching code by module name,
  source hash, and function id, patches module-constant slots, and falls back to
  lazy JIT when a function symbol is absent.
- `just precompile-shared-library counters=<profile.bin> out=<lib.so>`
  Offline precompiles all modules referenced by a counter dump from cached
  pre-optimization BlockPy modules, writes per-module object files, and links a
  shared library. The recipe regenerates `mod.opt` plans before compiling. It
  expects matching module-cache entries in `$SOAC_WORK_DIR/modules`; run a
  profile/benchmark pass first when the cache is empty. Use
  `SOAC_PRECOMPILED_LIBRARY` to point runtime execution at the resulting shared
  library.
- `cargo run -p soac-inspector --bin decide_optimizations -- --counters <profile.bin> --out <root-dir>`
  Standalone optimization-decision planner. It loads the counter dump once,
  scans cached BlockPy modules under the output root by default, and writes
  binary `mod.opt` artifacts beside those modules, such as
  `$SOAC_WORK_DIR/modules/python-stdlib/typing/mod.opt`. Use `--module` for a
  narrow debugging input or `--module-root` to scan a different cache root. Use
  `cargo run -p soac-inspector --bin print_optimization_plan -- --plan <mod.opt>`
  to pretty-print it for inspection. In `SOAC_OPT_MODE=verify|apply`, runtime
  specialization looks for a matching `mod.opt` in the active module cache and
  uses it for decision-backed call, operator, getitem, setitem, indexed-field,
  and branch specializations.
- `SOAC_CRANELIFT_OPT_LEVEL`
  Optional Cranelift process-JIT optimization level override:
  `none`, `speed`, or `speed_and_size`. Normal runtime and benchmark
  runs default to `speed`; `just pytest`, `just test-all`, and
  `just run-cpython-tests` default to `none` unless the caller already set it,
  so correctness runs do not spend cold-start time optimizing import-time helper
  code.
- `SOAC_JIT_COMPILE_WORKERS`
  Optional positive integer cap for worker threads used to compile functions
  inside one reserved process-JIT batch. Unset defaults to
  `min(available_parallelism, 4)`. Set `SOAC_JIT_COMPILE_WORKERS=1` to force
  single-worker compilation for before/after timing comparisons.
- `SOAC_BACKGROUND_JIT`
  Import-time background JIT compilation is enabled by default for ordinary
  runtime use. `just pytest`, `just pytest-fast`, and `just test-all` default it
  to `0` so correctness runs do not accumulate asynchronous compile work across
  many short-lived transformed modules. Set `SOAC_BACKGROUND_JIT=1` when a test
  or repro specifically needs the background compiler.
- `SOAC_JIT_EMIT_REFCOUNTS`
  Refcount emission is enabled by default. Set to `0`, `false`, `no`, or `off`
  to inline generated JIT INCREF/DECREF helper calls as no-ops. This is an
  intentionally unsound performance experiment knob and leaks Python
  references. It also disables guard-miss deopt replay, because the replay
  interpreter depends on normal owned-reference bookkeeping from generated code.
- `BEHAVIOR_CHANGE`
  Source comments with this exact tag mark intentional CPython-visible
  compatibility changes. Current examples: apply-mode raw indexed
  stores may skip dict/object/type observers, and undeclared
  known-builtin loads are lowered to `RuntimeName` constants rather than
  re-checking later module-global shadowing.
- `SOAC_MODULE_ENABLED`
  Optional comma-separated import-hook allow-list. Entries currently
  use `path:<file-or-directory>` and are resolved before matching. When
  set, `SoacLoader` only wraps source imports whose resolved source
  path is inside one of the listed roots. Test recipes intentionally do
  not set or change this variable; they inherit the caller environment
  unchanged.

### CPython-specific notes

- Vendored CPython lives at `vendor/cpython`.

- Only use `vendor/cpython/python` directly when there is no suitable
  `Justfile` recipe, or when debugging raw CPython rather than the
  built `_soac_ext` path.

- For `just run-cpython-tests 0 -f <file>`, pass an absolute path.
- In sandboxed environments, prefer `--tempdir /tmp/<dir>` for CPython test runs.
- After interrupting CPython regrtest workers, clean stale workers before retrying.

### Debugging aids

- To inspect transformed output quickly, run `cargo run --bin diet-python <file.py>`.
- For BB/JIT inspection, `cargo run -p soac-inspector --bin render_jit_clif -- <source> <function_id>`.
- To trace BB execution, set `SOAC_EXEC_TRACE` to `all`, `all:params`, `<exact-qualname>`, or `<exact-qualname>:params`.


### Jujutsu conventions

- Use `jj describe` with real newlines for multi-paragraph messages.
- Keep one logical change per `jj` change.
- After finishing a logical change and moving to the next, create a fresh child with `jj new`.
- Do not run mutating `jj` commands in parallel. Commands such as `jj describe`,
  `jj new`, `jj squash`, `jj split`, `jj rebase`, `jj abandon`, `jj restore`,
  `jj resolve`, and `jj bookmark set`/`move` rewrite repo state and can trigger
  automatic rebases; run them serially and verify with fresh `jj status`
  afterwards. Read-only inspection commands such as `jj diff`, `jj log`, and
  `jj show` may run in parallel when they do not depend on each other's output.
  Run `jj workspace update-stale` and `jj status` serially when using them as a
  preflight before mutation, so the mutation is based on fresh workspace state.
- For one-off revision checks, switch the current workspace in-place instead of
  creating another workspace or ad hoc temporary worktree. Create a new empty
  working-copy child at the revision you need:

  ```sh
  jj new <rev>
  ```

  Run the needed check from that revision, then move back to the previous work
  with another `jj new <rev>` or by editing the original change:

  ```sh
  jj edit <change-id>
  ```

- Before starting work or advancing `main`, run the multi-agent sanity check:

  ```sh
  jj workspace update-stale
  jj status
  jj log -r 'divergent() | (conflicts() & working_copies()) | (working_copies() ~ present(main)::) | (working_copies() ~ heads(working_copies()) ~ present(main))' --no-graph -T 'change_id.short() ++ " " ++ commit_id.short() ++ " " ++ description.first_line() ++ "\n"'
  ```

  The repo is in a valid multi-agent state if `jj status` does not report a
  stale or conflicted working copy and the `jj log` command above prints
  nothing.

- Default to the current workspace. For routine before/after validation,
  benchmark comparisons, and small isolated fixes, change the current
  workspace's `@` directly rather than creating a temporary jj workspace.
  Do not create ad hoc extra workspaces just to dodge unrelated local changes;
  prefer `jj new`, `jj edit`, `jj split`, or `jj diff` to keep the intended
  change isolated. Use a separate workspace only when concurrent active work,
  destructive experimentation, or strict isolation is actually required.
- If a separate workspace is genuinely required, make its setup explicit before
  validation that depends on vendored CPython, shared caches, or a staged
  `_soac_ext`. Run `just setup-dev-env` there first rather than assuming the
  sibling workspace is already provisioned.

  The revset flags four invalid states:
  - `divergent()`: there is unresolved divergence in visible changes.
  - `conflicts() & working_copies()`: a live workspace is currently conflicted.
  - `working_copies() ~ present(main)::`: a live workspace is not based on `main`.
  - `working_copies() ~ heads(working_copies()) ~ present(main)`: one live
    workspace is an ancestor of another live workspace, which means someone is
    depending on another workspace's mutable `@` instead of depending only on
    `main`. The `~ present(main)` exception allows a workspace to sit directly on
    `main`.

### General communications 

- If I say that some approach is bad or distatestful, extract a
  generalizable design principle that captures that decision.  Confirm
  that with me, then add to AGENTS.md

- When pointing at code, include both the name of the enclosing item
  as well as the file and line number.  e.g don't just refer to a file
  and `src/foo/bar.rs:124`, say `in struct FooBar, at
  src/foo/bar.rs:124`.
