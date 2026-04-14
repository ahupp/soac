# soac-blockpy/src/passes/blockpy_generators/mod.rs

## File Responsibilities
Lowers generator, coroutine, and async-generator functions from yield-capable BlockPy into explicit factory/resume functions and runtime closure objects. It computes persistent generator state, resume ABI parameters, yield-site dispatch, cleanup behavior, injected exception handling, and `yield from`/async generator protocols.

## Datatypes
- `ResumeAbiParam`: enum for hidden resume parameters (`send`, `throw`, `close`, async-generator value) and their canonical names.
- `GENERATOR_RESUME_ABI_PARAMS` / `ASYNC_GENERATOR_RESUME_ABI_PARAMS`: fixed hidden parameter lists for resume functions.
- Type aliases `LinearYieldStmt`, `LinearCoreStmt`, `LinearYieldBlock`, `LinearCoreBlock`, and `BlockPyBlock`: clarify intermediate block shapes.
- `ErrOnYield`: mapper used when lowering expressions that must not contain yield.
- `ResumeClosureBindings`: records closure/cell bindings needed by generated resume functions.
- `YieldSite`: identifies yield/yield-from/await/async-yield suspension points.

## Functions
- No-yield conversion helpers: `try_lower_core_expr_without_yield_with_mapper`, `lower_stmt_no_yield`, and `lower_term_no_yield` convert yield-capable nodes when suspension is not allowed.
- Resume ABI/name helpers: `resume_abi_params`, `generator_state_logical_name`, `generator_state_storage_name`, `runtime_init`, `unresolved_name`, `core_name`, and store/load constructors build synthetic names and statements.
- State analysis helpers: `collect_state_vars`, `assigned_names_in_linear_stmt`, `assigned_names_in_term`, `collect_named_expr_target_names`, `injected_exception_names`, `build_generator_storage_layout`, `persistent_generator_state_order`, and `generator_cleanup_cell_logical_names` determine what survives suspension.
- Function/factory builders: `core_generator_code`, `core_make_function`, `build_factory_block`, `resume_param_spec`, `generator_resume_declared_params`, and related parameter-index helpers construct generated callable definitions.
- Yield-site helpers: `stmt_yield_site`, `term_yield_site`, `yield_value_expr`, `completion_raise`, `push_completion_raise_block`, `explicit_jump_args_for_params`, and resume-condition helpers build the resume control-flow graph.
- Protocol helpers for `yield from`, await, throw, close, StopIteration, and async-generator send/throw/close construct runtime calls and exception paths.
- The public lowering entry points in this module transform generator-like functions into factory functions plus resume functions and update the module callable list.

## Context Read
- `soac-blockpy/src/passes/blockpy_generators/suspend_order.rs` for suspension ordering.
- `soac_py/src/soac/runtime.py` generator/coroutine runtime classes and helpers.
- `soac-blockpy/src/block_py/scope.rs` for storage layout and callable scope information.
