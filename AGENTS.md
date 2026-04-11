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

4. Keep payload nodes context-free.  Nested IR payloads like names and
   literals should represent only the payload itself; operation kind
   and source metadata belong on the enclosing IR node such as
   `Load`/`Store`/`Del` or `LiteralValue`, not duplicated inside the
   payload.

5. Lower source names in stages. A raw AST `ExprName` may only lower
   directly to `UnresolvedName`. `LocatedName` must only come from an
   explicit name-binding or other explicit location decision, never
   from a blind default location.

6. Avoid global mutable state; if needed there should be a single
   global structure and then all consumers take that structure rather
   than directly accessing the global.

7. Keep SOAC metadata explicit across CPython callback boundaries.
   Do not smuggle compiler/runtime state through temporary Python
   attributes or thread-local context just to reach a fixed-signature
   CPython hook; prefer an explicit post-create/post-callback init step
   or a clearly-owned Rust state object.

8. Keep `soac-runtime` visibly raw and ABI-shaped. It is the very-hot
   local runtime layer that gets inlined into generated code: avoid PyO3
   wrapper types there, name hand-written CPython layout mirrors with a
   `RawPy*` prefix, and keep casts clustered at ABI boundaries so direct
   layout access remains obvious in review.

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
   If diagnosing a hang, add follow-up instrumentation where practical and leave behind a focused regression or assertion for that hang shape.
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

8. Keep environment-variable docs in sync.

   If you add or materially change an environment variable that controls
   runtime behavior, testing, benchmarking, profiling, or the local web
   tooling, document it in `README.md` and add or update the relevant
   note in `AGENTS.md` in the same logical change.

9. Record finalized performance changes.

When a change is expected to affect performance, validate it with a before/after
benchmark before treating it as complete. Use the repo benchmark comparison
workflow unless there is a specific reason to use a narrower measurement, and
report both headline throughput and relative delta.

When a performance change is complete enough that you intend to keep it,
rebase the finished change onto `main`, run `$soac-profile-benchmark`, write the
finalized benchmark result to `bench/{change_id}`, and
append an entry to `docs/CODEX_OPT_LOG.md` in the same logical change.
Use `bench/{change_id}_{commit_id}` only for one-off test benchmarks while
iterating. Keep log entries succinct: include the jj change id, a short summary
of the optimization, the benchmarked throughput delta, and the before/after
headline numbers. Do not paste validation checklists, full command lines, or
long run logs.

10. Run the full gate before submitting code changes.

Run `just test-all` before submitting unless the change is docs-only,
such as `TODO.md`, `AGENTS.md`, or similar documentation-only files.
Put test output in `logs/`. Summarize the failures, separate expected
failures from unexpected failures, investigate the root cause, report
it, then fix it.

11. When a logical set of changes is complete, freeze it before
   integrating it.

Run `jj new` so the finished work is no longer the live working commit.
Rebase and integrate the finished change, not the live `@`.

12. Try to advance `main` directly to the finished head.

Prefer `jj bookmark move main -t <finished-head>` when the finished
change is already a descendant of the current `main`. This avoids
unnecessary rebases and duplicate sibling revisions.

13. If advancing `main` fails because the finished head is not a
   descendant of `main`, rebase the finished commit or finished stack
   onto `main`.

Use `jj rebase` on the finished revision or stack root so the completed
work sits directly on top of the current shared base.

14. Resolve any conflicts and rerun the relevant tests.

The rebased change is not ready to advance `main` until conflicts are
resolved and the relevant checks have been rerun.

If conflict recovery involved restoring files from a known-good pre-rebase
revision, inspect `jj diff --summary <good-rev>..main` and restore the entire
logical file set before validating. Partial restores can leave the change in an
internally inconsistent state that compiles or fails for misleading reasons.

15. Advance `main` to the finished head.

This is the synchronization point. Once `main` moves, the finished work
becomes the new shared base for future work.

16. When another agent advances `main`, refresh and continue on top of
    it.

Run `jj workspace update-stale` and rebase your live work onto the new
`main` as needed. Other agents should only depend on `main`, not on a
peer workspace's live `@`.

17. Report the result: run `jj diff --stat` on the completed change and
report its output, then describe the next step. If I did not ask to
approve each step after the plan, continue with the next step.

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
`Environment notes: none`.


## APPENDIX

### Testing and runtime entrypoints

- `just test-all`
  Full gate for non-doc changes.
- `just pytest ...`
  Authoritative transformed-runtime pytest entrypoint.
- `just py ...`
  Best entrypoint for ad hoc transformed-runtime repros outside pytest.
- `just run-cpython-tests ...`
  Use for vendored CPython regrtest runs.
- `$soac-profile-benchmark`
  Default skill for performance benchmark requests. This uses the
  default pystone workflow: profile, verify, and specialized apply benchmark.
  One-off test benchmarks write `bench/{change_id}_{commit_id}`. Finalized
  benchmarks for changes that are being merged to `main` must run after rebasing
  onto `main` and write `bench/{change_id}`. Use `just benchmark-deep-profile`
  when the user explicitly wants inspector/CLIF artifacts or perf capture, and
  use `just benchmark-deep-profile-from-profile <result-dir>` to extend an
  existing `counters/profile.bin` result without rerunning the profile pass.
  Report the specialized apply-pass median from the result directory's
  `benchmark.txt` unless I explicitly ask for the warm unspecialized baseline.
- Repo-local uv state
  `.envrc` and `Justfile` keep uv and XDG state under the repo with
  `UV_CACHE_DIR`, `UV_TOOL_DIR`, `UV_TOOL_BIN_DIR`, `XDG_CACHE_HOME`,
  `XDG_DATA_HOME`, and `XDG_RUNTIME_DIR`. The `Justfile` respects pre-set values
  for those variables. `just setup-dev-env` installs the repo-local `ruff`
  command. Test and benchmark recipes use `UV_OFFLINE=1` for uv-backed venv
  refreshes; use `just update-venv` or rerun `just setup-dev-env` when
  dependency changes intentionally require network access.
- `SOAC_PARENT_REPO`
  Optional override for `just setup-dev-env` in a jj worktree. The setup recipe
  normally infers the parent checkout from a file-backed `.jj/repo`; the parent
  owns shared offline state and `bench/` as a regular directory. The setup
  recipe symlinks `vendor/cpython`, `bench/`, `.uv-cache`, `.uv/`, `.xdg/`, and
  `tmp/cargo-home` from that parent into the worktree, and errors instead of
  creating isolated empty offline caches when neither inference nor the override
  can identify the parent.
  When sandboxing would otherwise block shared benchmark writes, run Codex with
  the worktree and parent checkout as writable roots, for example
  `--add-dir ../main-repo .`.
- `SOAC_WORK_DIR` / `SOAC_OPT_MODE`
  Normal specialization runs use one work directory with conventional
  files: `profile.bin` for specialization input, `verify.bin` for the
  countered verification pass, and `events.jsonl` for default JSON
  tracing output. Set `SOAC_OPT_MODE=none`, `profile`, `verify`, or
  `apply`; recipes should pass the same `SOAC_WORK_DIR` and change only
  the mode between passes. `none` is the explicit ordinary
  unspecialized/no-counter mode and should not read or write counter
  dumps. `apply` still skips counter dump files, but when event logging
  is enabled it records indexed specialization hit/fallback counts long
  enough to emit `soac_specialization_runtime` summary events.
- `SOAC_ENABLE_PROFILED_COLD_BLOCKS`
  Optional opt-in for replaying `block_entry` counters from
  `$SOAC_WORK_DIR/profile.bin` as Cranelift `cold` block hints during
  `verify`/`apply`. This is disabled by default; the counters remain
  recorded in `profile`/`verify` either way.
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
  summaries are emitted by `soac_specialization_runtime`. When
  `SOAC_LOG` is unset and `SOAC_WORK_DIR` is set, the default event log
  is `$SOAC_WORK_DIR/events.jsonl` and includes
  `soac_specialization_runtime`.
- `SOAC_PYTEST_TRACE` / `SOAC_PYTEST_EVENTS_LOG`
  `just pytest ...` and `just test-all` do not write the verbose JSON module
  load/JIT-codegen trace by default. Set `SOAC_PYTEST_TRACE=1` to enable it
  when `SOAC_LOG` is unset. `SOAC_PYTEST_EVENTS_LOG` overrides the output path,
  which defaults to `logs/pytest_events.jsonl`.
- `SOAC_RUN_SLOW_TESTS`
  `just pytest ...` and `just test-all` deselect pytest tests marked `slow` by
  default. Set `SOAC_RUN_SLOW_TESTS=1` to include intentionally expensive tests
  such as broad import-hook coverage. Direct pytest invocations can also pass
  `--run-slow`.
- `SOAC_CRANELIFT_OPT_LEVEL`
  Optional Cranelift process-JIT optimization level override:
  `none`, `speed`, or `speed_and_size`. Normal runtime and benchmark
  runs default to `speed`; `just pytest`, `just test-all`, and
  `just run-cpython-tests` default to `none` unless the caller already set it,
  so correctness runs do not spend cold-start time optimizing import-time helper
  code.
- `SOAC_CRANELIFT_COMPILE_CACHE`
  Set to `1`, `true`, `yes`, or `on` to enable the experimental
  filesystem-backed Cranelift incremental compile cache. It writes
  entries to `SOAC_COMPILE_CACHE_DIR`, `$SOAC_WORK_DIR/compile-cache`,
  or a process temp directory using key-derived filenames, and logs
  cache configuration, hits, and store failures through the
  `soac_jit_compile_cache` tracing target. The cache is disabled by
  default. Direct Python function bodies are currently skipped because
  their Cranelift input still embeds per-run object and counter pointers.
- `SOAC_COMPILE_CACHE_DIR`
  Explicit cache directory for `SOAC_CRANELIFT_COMPILE_CACHE`. Prefer
  this for CPython test runs and symlinked/shared checkout workflows so
  cache writes do not depend on the process current directory.
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
- Treat "do not run `jj` workspace inspection commands in parallel" as a hard rule.
  Even read-only repo-state commands such as `jj status`, `jj workspace update-stale`,
  and `jj log` can race on snapshotting or auto-recovery in agent workflows. Run
  them serially, wait for completion, then inspect the next command's fresh output.
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
