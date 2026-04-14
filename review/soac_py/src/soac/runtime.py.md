# soac_py/src/soac/runtime.py

## File Responsibilities

Python-level runtime support module for lowered SOAC programs. It exposes runtime constants, builtin aliases, operation helpers, function/class construction helpers, generator/coroutine/async-generator wrappers, import helpers, exception helpers, pattern matching helpers, context-manager helpers, and fallback implementations for source constructs not emitted directly in JIT code.

## Datatypes

- Runtime constants: `NO_DEFAULT`, `ELLIPSIS`, `TRUE`, `FALSE`, `NONE`, `EMPTY_TUPLE`, `_SOAC_RUNTIME_READY`, `_DP_CODE_WITH_FREEVARS_CACHE`, `_CLIF_ENTRY_RUNTIME_ERROR`, and typing/template aliases.
- `AsyncGenComplete`: private control-flow exception carrying async-generator completion values.
- `ClosureGenerator`: wrapper implementing generator protocol for closure-backed lowered generator bodies.
- `Coroutine`: wrapper adapting `ClosureGenerator` to coroutine protocol and introspection properties.
- `ClosureAsyncGenerator`: wrapper implementing async-generator protocol for closure-backed lowered async generators.
- `AsyncGenSend`: awaitable/send object driving one async-generator send/throw/close operation.
- `_AwaitIterWrapper`: minimal object whose `__await__` returns a supplied iterator.

## Functions and Methods

- Builtin/runtime aliases: binds selected builtins, typing objects, and `soac.sim` operation helpers into stable runtime names.
- `_unsupported_frame_builtin`: raises for frame-sensitive builtins (`locals`, `eval`, `exec`) that transformed code cannot support directly.
- `tuple_values`, `tuple_from_iter`, `eval_string_literal`, `__deepcopy__`, `templatelib_Template`, and `templatelib_Interpolation`: simple construction/evaluation helpers used by lowered code.
- `load_deleted_name`: raises when a deleted-name sentinel is loaded.
- `bb_trace_enter`: optional basic-block trace printer controlled by `SOAC_EXEC_TRACE`.
- Generator support helpers: `_yieldfrom_cell_value`, `_current_yieldfrom`, `_is_cancelled_error`, `_reraise_control_flow`, `_clear_cell`, `_mark_closed`, `_normalize_throw_exc`, and `_current_throw_context`.
- `ClosureGenerator` methods: initialize generator state, implement iteration/send/throw/close, propagate cancellation/control-flow, and expose `gi_yieldfrom`.
- `Coroutine` methods/properties: forward await/iteration/send/throw/close to the backing generator and expose coroutine introspection.
- `ClosureAsyncGenerator` methods/properties: implement async iteration, create `AsyncGenSend` awaitables, forward selected generator attributes, and expose `gi_yieldfrom`.
- `AsyncGenSend` methods: implement awaitable iteration, perform send/throw/close stepping, translate completion to `StopIteration`, and guard re-use.
- Numeric/class helpers: `float_from_literal`, `complex_from_parts`, `class_lookup_cell`, and `class_lookup_global`.
- Exception helpers: `_validate_exception_type`, `exception_matches`, `exceptiongroup_split`, `exc_info`, `exc_info_from_exception`, `current_exception`, `raise_from`, and `_call_exception_class`.
- Iterable/control helpers: `unpack`, `_AwaitIterWrapper.__init__`, `_AwaitIterWrapper.__await__`, `_get_awaitable_iter`, `await_iter`, and `next_or_sentinel`.
- Super and pattern helpers: `call_super`, `call_super_noargs`, `_match_class_validate_arity`, `match_class_attr_exists`, and `match_class_attr_value`.
- Function/class construction helpers: `code_with_freevars`, `_entry_template`, `code_template_gen`, `annotation_forwardref_value`, and `create_class`.
- Import helpers: `import_`, `import_attr`, and `import_star`.
- Context-manager helpers: `_lookup_special_method`, `_has_special_method`, `_missing_context_protocol_message`, `contextmanager_enter`, `contextmanager_get_exit`, `contextmanager_exit`, `_ensure_awaitable`, and `asynccontextmanager_get_aexit`.

## Context Read

- `soac-pyo3/src/jit_runtime.rs`
- `soac_py/src/soac/sim.py`
- `docs/RUNTIME_FUNCTIONS.md`

