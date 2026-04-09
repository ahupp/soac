
We need to add support for generators.  At the moment, in rewrite_module_with_tracker, the value `core_blockpy_without_await` contains functions that have 'await' reduced to
`yield from _dp_some_helper`.  We need to reduce those to regular functions that implement the generator protocol

It is very important, most important, that the generator transform only consume that value as input (a BlockPyModule<CoreBlockPyPassWithoutAwait>), and returns a BlockPyModule<CoreBlockPyPassWithoutAwaitOrYield>.

If there is insufficient information in the input, stop and describe the issue before proceeding.

The structure of generators will be to have a new, generated, outer function (closure) that holds all the internal state in cells, as well as all locals of the generator.  This function will be "resume", and take two values, "send_value" and "throw_value", where at most one is non-none.  The most important cell is "_dp_pc", the program counter indicating which yield point we're on.

Split the generator blocks at yield points, and map each resume point to a PC.  Then, a yield looks like:

_dp_pc = <next pc>
return value

throw is similar, but throws in a block wrapped by the corresponding exception block.  Be sure to handle all the nuances of generators with e.g throwing an exception, StopIteration etc.

Concrete implementation plan
============================

The current code already has three useful pieces we should reuse instead of inventing a second lowering story:

1. `BlockPyModule<CoreBlockPyPassWithoutAwait>` is the exact pre-generator input boundary we want.
2. `blockpy_generators::build_blockpy_closure_layout(...)` already computes the closure/runtime-cell layout we need for closure-backed resumptions.
3. `soac_py/src/soac/runtime.py` already has the closure-backed runtime objects:
   - `def_hidden_resume_fn(...)`
   - `make_closure_generator(...)`
   - `make_coroutine_from_generator(...)`
   - `make_closure_async_generator(...)`

So the transform should stay entirely inside the `core_blockpy_without_await -> core_blockpy_without_await_or_yield` step and should expand generator-like callables into explicit regular functions.

Step-by-step plan
-----------------

1. Preserve the real callable kind before the no-yield boundary.
   - Detect `yield` / `yield from` while lowering function defs from Ruff AST.
   - Set kinds as:
     - sync def without yield -> `Function`
     - sync def with yield -> `Generator`
     - async def without yield -> `Coroutine`
     - async def with yield -> `AsyncGenerator`
   - Stop erasing that distinction in `build_lowered_blockpy_function_bundle(...)`.

2. Replace the current no-yield panic-only pass with a real module transform.
   - Keep the current by-value module input/output types:
     - input: `BlockPyModule<CoreBlockPyPassWithoutAwait>`
     - output: `BlockPyModule<CoreBlockPyPassWithoutAwaitOrYield>`
   - Non-generator callables should continue to use the existing structural `try_into()` path.
   - Generator-like callables should expand into:
     - a visible factory callable with the original bind name / display name / qualname
     - a hidden resume callable that carries the original state machine body

3. Build the visible factory callable explicitly.
   - Reuse the existing closure-layout metadata from the semantic/lowered callable.
   - Allocate local/runtime cells with `__dp_make_cell(...)`.
   - Build the hidden resume entry with `__dp_def_hidden_resume_fn(...)`.
   - Return:
     - `__dp_make_closure_generator(...)` for sync generators
     - `__dp_make_coroutine_from_generator(__dp_make_closure_generator(...))` for coroutines
     - `__dp_make_closure_async_generator(...)` for async generators
   - Keep the visible callable kind as `Generator` / `Coroutine` / `AsyncGenerator` so later rendering and metadata stay honest.

4. Lower the original callable body into a hidden resume function.
   - Linearize structured `if` fragments first so the resume transform only has to deal with flat CFG blocks.
   - Introduce a dispatch entry block that branches on `_dp_pc`.
   - Assign one PC to the original entry block and one PC to every continuation after a suspension point.
   - Reuse the original exception-edge metadata for split continuation blocks so `throw(...)` re-enters the right exception regions.

5. Lower plain `yield` explicitly.
   - For each `yield`, split the current block into:
     - pre-yield code
     - a returned yield value path
     - a continuation resume point
   - On the yield path:
     - set `_dp_pc` to the continuation PC
     - return the yielded value
   - On the continuation path:
     - if `_dp_resume_exc` is not `None`, raise it inside the original exception region
     - otherwise, for `x = yield y`, bind `_dp_send_value` into `x`
     - continue with the remainder of the original block

6. Lower generator/coroutine completion explicitly.
   - A plain fallthrough or `return value` in the hidden resume function must not become a normal function return.
   - For sync generators and coroutines, lower completion to `raise StopIteration(value)`.
   - For async generators, lower completion to `raise StopAsyncIteration`.
   - After normal completion, set `_dp_pc` to a terminal PC so repeated resumes stay exhausted.

7. Lower `yield from` with a small dedicated runtime helper instead of re-encoding all of PEP 380 in CFG.
   - Add one helper in `soac_py/src/soac/runtime.py` for sync-generator/coroutine delegation:
     - step the delegated iterator using `next` / `send` / `throw`
     - return whether the delegate completed and the yielded/final value
   - In the CFG lowering:
     - initialize `_dp_yieldfrom`
     - resume through a dedicated delegation block
     - if the helper says “not done”, return the yielded value and keep the same PC
     - if the helper says “done”, clear `_dp_yieldfrom`, bind the final value, and continue
   - This keeps the transform-time structure explicit while isolating the fiddly iterator protocol details in one runtime helper.

8. Stage async-generator support after the sync generator/coroutine path is stable.
   - The same outer factory + resume split applies.
   - The hidden resume function needs `_dp_transport_sent` threaded through its state order.
   - Async-generator `yield` lowering needs a dedicated transport/yield helper in `soac_py/src/soac/runtime.py`, because `_DpAsyncGenSend` interprets `_dp_yieldfrom` specially.
   - Keep this as a second implementation slice if the sync/coroutine path lands cleanly first.

9. Test in layers.
   - Focused lowering/render tests:
     - `lowers_generator_yield_to_explicit_blockpy_dispatch`
     - `lowers_generator_yield_from_to_explicit_blockpy_dispatch`
     - `lowers_async_generator_yield_to_explicit_blockpy_dispatch`
   - Focused runtime regressions:
     - `tests/test_regression_sync_generator_stop.py`
     - `tests/test_regression_sync_generator_throw.py`
     - `tests/test_regression_coroutine_return_value.py`
   - Then run `just test-all` and compare the remaining failures against the current generator/no-yield baseline.

Implementation order for the actual code change
-----------------------------------------------

1. Preserve callable kinds in function lowering.
2. Promote the generator-factory helper from `blockpy_generators` tests into production code.
3. Replace the no-yield panic transform with:
   - a structural pass-through path for plain functions
   - a generator-expansion path for sync generators and coroutines
4. Add the small sync `yield from` runtime helper and wire it into the generator-expansion path.
5. Re-run the focused generator tests.
6. Extend the same machinery to async generators once the sync/coroutine slice is stable.
