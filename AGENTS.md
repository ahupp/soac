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

4. Regenerate generated artifacts through the standard entrypoints.
   If fixtures fail, regenerate snapshots with `just regen-snapshots`.
   Keep real snapshot updates in the same logical change so regressions remain visible in review.
   Check `snapshot/snapshot_summary.txt` for surprising BlockPy or CLIF count shifts.

5. Add focused regression coverage for real bugs.
   If fixing a CPython regression, add a minimal reproducing integration test first.
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
append an entry to `docs/CODEX_OPT_LOG.md` in the same logical change.
Keep entries succinct: include the jj change id, a short summary of the
optimization, the benchmarked throughput delta, and the before/after
headline numbers. Do not paste validation checklists, full command lines,
or long run logs.

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
- `just regen-snapshots`
  Regenerates fixture snapshots.
- `just benchmark`
  Default benchmark for performance requests. This is the artifact-producing
  pystone workflow: profile, verify, specialized apply benchmark, textual
  counter/specialization dumps, and perf for the specialized apply run. It
  prints `benchmark result: bench/{change_id}_{commit_id}`; report the
  specialized apply-pass median from that directory's `benchmark.txt` unless I
  explicitly ask for the warm unspecialized baseline.
- `SOAC_WORK_DIR` / `SOAC_OPT_MODE`
  Normal specialization runs use one work directory with conventional
  files: `profile.bin` for specialization input, `verify.bin` for the
  countered verification pass, and `events.jsonl` for default JSON
  tracing output. Set `SOAC_OPT_MODE=none`, `profile`, `verify`, or
  `apply`; recipes should pass the same `SOAC_WORK_DIR` and change only
  the mode between passes. `none` is the explicit ordinary
  unspecialized/no-counter mode and should not read or write counter dumps.
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
  `soac_jit_codegen`. When `SOAC_LOG` is unset and `SOAC_WORK_DIR` is
  set, the default event log is `$SOAC_WORK_DIR/events.jsonl`.
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
- Before starting work or advancing `main`, run the multi-agent sanity check:

  ```sh
  jj workspace update-stale
  jj status
  jj log -r 'divergent() | (conflicts() & working_copies()) | (working_copies() ~ present(main)::) | (working_copies() ~ heads(working_copies()) ~ present(main))' --no-graph -T 'change_id.short() ++ " " ++ commit_id.short() ++ " " ++ description.first_line() ++ "\n"'
  ```

  The repo is in a valid multi-agent state if `jj status` does not report a
  stale or conflicted working copy and the `jj log` command above prints
  nothing.

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
