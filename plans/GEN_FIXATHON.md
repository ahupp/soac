# Generator / Await Fixathon

Investigation log for the transformed-runtime failure cluster that was present on 2026-03-20.

## Final Status

- Final validation: `just test-all` passed on 2026-03-20.
- Final log: `logs/just-test-all-20260320-gen-fixathon-final.log`
- Scope:
  - the original generator / await failure cluster
  - the remaining `locals()` / `exec()` regressions uncovered after the generator fixes
  - one additional class-scope regression uncovered during the sweep

## Root-Cause Reports

### 1. Broken sync `with` lowering

Tests covered:
- `tests/test_regression_sync_generator_throw.py::test_sync_generator_throw_handles_except_name_cleanup`
- `tests/test_regression_contextmanager_special_lookup.py::test_with_bypasses_getattribute_for_specials`
- `tests/test_regression_with_enter_result_lifetime.py::test_with_enter_result_is_not_retained`
- `tests/test_regression_method_named_open.py::test_method_named_open_uses_builtin`
- `tests/test_cpython_transform_failures.py::test_method_named_open_calls_builtin`
- corresponding transformed integration duplicates

Root cause:
- The specialized sync-`with` lowering path in BlockPy built an incorrect cleanup sequence.
- It could preserve an already-suppressed abrupt exception, and it could also null out the saved exit callable before the final `__exit__` call.
- That one bug family surfaced as wrong suppression behavior, broken special-method lookup, premature retention failures, and `NoneType is not callable` in ordinary `with open(...)`-style cases.

Proposed fix:
- Delete the bespoke sync-`with` lowering path.
- Route sync `with` through the existing structured desugar that already models suppression and special lookup correctly.

Landed fix:
- Removed the special-case path and reused `desugar_structured_with_stmt_for_blockpy(...)`.
- Main file: `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/with_stmt.rs`

Status:
- fixed

### 2. Finally-edge live-ins clobbered recursion / current-exception state

Tests covered:
- `tests/test_regression_recursive_local_function.py::test_recursive_local_function_keeps_closure_cell_binding`
- `tests/test_regression_current_exception_recursion.py::test_current_exception_recursion_regression`

Root cause:
- Synthetic jumps into finally handlers over-specified the argument payload for the target block.
- That let internal finally bookkeeping clobber ordinary live-ins, including the recursive closure cell and current-exception state that the handler still needed.

Proposed fix:
- Make finally dispatch preserve the real live-in set instead of re-materializing a wider synthetic payload.

Landed fix:
- Tightened finally exception/live-in handling in:
  - `soac-blockpy/src/block_py/exception.rs`
  - `soac-blockpy/src/passes/ruff_to_blockpy/try_regions.rs`
  - `soac-blockpy/src/passes/ruff_to_blockpy/module_plan.rs`

Status:
- fixed

### 3. Async-generator protocol errors leaked through the wrong boundary

Tests covered:
- `tests/test_regression_asyncgen_anext_send_non_none.py::test_asyncgen_anext_send_non_none_raises_type_error[transform]`
- `tests/test_regression_async_contextmanager_stopiter.py::test_async_contextmanager_stopiter_regression`

Root cause:
- `_DpAsyncGenSend.send()` did not enforce the CPython rule that the first `send()` into `agen.__anext__()` must be `None`.
- The same awaitable boundary also let completion exceptions escape with the wrong shape, so async-generator protocol failures were observed as normal completion or as the wrong exception kind.

Proposed fix:
- Enforce the “first send must be `None`” rule in `_DpAsyncGenSend`.
- Normalize async-generator completion/protocol exceptions at the awaitable boundary instead of letting them leak out raw.

Landed fix:
- Runtime updates in `soac_py/src/soac/runtime.py`

Status:
- fixed

### 4. Coroutine completion was routed through user exception handlers

Tests covered:
- `tests/test_regression_await_return_passthrough.py::test_await_uses_coroutine_result_not_stopiteration`

Root cause:
- Synthetic coroutine-completion raises were emitted in blocks that still inherited the surrounding user `exc_target`.
- That let user `except Exception` handlers intercept internal completion signaling and reinterpret it as ordinary program flow.

Proposed fix:
- Emit synthetic completion raises in a fresh block with `exc_target = None`.

Landed fix:
- Added a dedicated completion-raise block in `soac-blockpy/src/passes/blockpy_generators/mod.rs`

Status:
- fixed

### 5. Async-generator resume state and exception dispatch were inconsistent

Tests covered:
- `tests/test_regression_async_genexpr.py::test_async_genexpr_with_async_listcomp`
- `tests/test_regression_asyncio_call_graph_async_gen.py::test_asyncio_call_graph_handles_async_gen`
- async-comprehension / nested-async-comprehension transformed integration duplicates

Root cause:
- JIT exception-dispatch planning stored filtered parameter indexes and later applied them against unfiltered parameter layouts.
- That corrupted resume locals when an exception edge was taken.
- Separately, async-generator completion reused user-visible `StopAsyncIteration`, so the runtime could not distinguish “generator is done” from “user code raised `StopAsyncIteration`”.

Proposed fix:
- Track exception-dispatch sources by name, not by filtered runtime index.
- Introduce an internal async-generator completion signal and translate it to `StopAsyncIteration` only at the public async-generator boundary.

Landed fix:
- JIT planning/runtime updates in:
  - `soac-jit/src/jit/planning.rs`
  - `soac-jit/src/jit/mod.rs`
- async-generator completion updates in:
  - `soac-blockpy/src/passes/blockpy_generators/mod.rs`
  - `soac_py/src/soac/runtime.py`

Status:
- fixed

### 6. Timeout cancellation retained generator payloads

Tests covered:
- `tests/test_regression_asyncio_wait_for_release.py::test_wait_for_timeout_releases_payload`
- corresponding transformed integration duplicate

Root cause:
- There were two leaks:
  - `CancelledError` / `GeneratorExit` re-raise paths kept traceback chains alive longer than necessary
  - JIT vectorcall trampoline state kept extra references to per-call argument tuples

Proposed fix:
- Clear owner state aggressively on terminal generator paths.
- Strip control-flow tracebacks on the internal cancellation/close paths.
- Fix the JIT refcount imbalance around vectorcall-built argument tuples.

Landed fix:
- Runtime cleanup in `soac_py/src/soac/runtime.py`
- JIT refcount fixes in `soac-jit/src/jit/mod.rs`

Status:
- fixed

### 7. `eval()` / `exec()` / `locals()` visibility disagreed about closure state

Tests covered:
- `tests/test_regression_eval_closure.py::test_eval_sees_closure_cells`
- `tests/test_regression_exec_locals.py::test_exec_sees_locals`
- `tests/test_integration_cases.py::test_integration_case[transformed-locals_cell_contents]`
- `tests/test_integration_cases.py::test_integration_case[transformed-scope_locals]`

Root cause:
- The JIT had a special fast-path for `__dp_locals()` / `__dp_dir_()` that synthesized a locals mapping from filtered runtime block params.
- That dropped ambient closure storage such as inherited `_dp_cell_*` values.
- Bare `exec(...)` / `eval(...)` also relied on the helper discovering caller locals from the Python frame, which does not faithfully represent BB locals under the JIT wrapper path.
- There was an earlier related fast-path for `__dp_eval_` / `__dp_exec_` that bypassed the runtime helper entirely and lost closure cells.

Proposed fix:
- Keep `__dp_eval_` / `__dp_exec_` on the normal helper path.
- Teach the JIT locals fast-path to include ambient closure storage.
- Rewrite bare `exec(...)` / `eval(...)` calls so they pass an explicit `__dp_locals()` mapping when they intend to use the caller’s default local scope.

Landed fix:
- Removed the JIT `__dp_eval_` / `__dp_exec_` shortcut in `soac-jit/src/jit/mod.rs`
- Included ambient values in JIT locals synthesis in `soac-jit/src/jit/mod.rs`
- Rewrote one-argument `exec(...)` / `eval(...)` to pass `None, __dp_locals()` in `soac-blockpy/src/passes/ast_to_ast/rewrite_names.rs`
- Kept `_default_visible_locals(...)` support in `soac_py/src/soac/runtime.py`

Status:
- fixed

### 8. Nested class-body sync could overwrite the wrong binding

Tests covered:
- `tests/test_cpython_transform_failures.py::test_nested_class_getattribute_captures_outer_bindings`

Root cause:
- Class-body cell-sync emission still treated nested `ClassDef` statements like stale local-binding sync sites.
- That let a stale outer binding leak into nested class behavior.

Proposed fix:
- Stop emitting local cell-sync statements for nested `ClassDef`.

Landed fix:
- Removed `ClassDef` from `stmt_cell_sync_stmts()` in `soac-blockpy/src/passes/ast_to_ast/rewrite_names.rs`
- Added a focused unit test for the stale-sync shape

Status:
- fixed

## Validation

- Focused generator / async repros were rerun after each cluster fix.
- Focused locals / exec rerun:
  - `logs/pytest-focused-locals-after-fix-20260320.log`
- Final suite:
  - `logs/just-test-all-20260320-gen-fixathon-final.log`

## Result

- All failures from the original generator / await cluster are fixed.
- The follow-on `locals()` / `exec()` regressions uncovered during the sweep are fixed.
- `just test-all` is green at the end of the change.
