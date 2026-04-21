# crates/soac_jit/src/jit/deopt_interpreter.rs

## File Responsibilities

Implements the cold runtime interpreter used after JIT deoptimization. Given a deopt invocation, it materializes live local
state, resumes at a recorded BlockPy cursor, interprets supported BlockPy instructions/terminators with CPython C API calls,
and returns an owned Python result or a null pointer with a Python exception set.

## Datatypes

- CPython extern declarations: method/cell helpers, raised-exception setter, kwargs merge, and kwargs error formatter used
  to reproduce Python call semantics.
- `BlockPyDeoptFrame<'inv, 'data>`: active deopt interpreter frame, holding the invocation, materialized locals, and the
  currently captured exception object.

## Functions

- Entry/lifetime: `execute_deopt_invocation`, `BlockPyDeoptFrame::new`, `execute`, `execute_from_cursor`, and
  `release_frame_owned_values` run a deopt continuation and release owned locals/current exception.
- Control flow and exceptions: `try_dispatch_exception_edge`, `capture_current_exception_for_dispatch`,
  `execute_jump_edge`, `current_exception_arg_owned`, and `take_current_raised_exception_owned` move through block edges and
  preserve current-exception state.
- Block arguments and local/cell access: `execute_block_arg_name_owned`, `execute_load_owned`, `execute_cell_load_owned`,
  `execute_return_local`, `execute_module_constant`, `execute_raw_cell_object_for_location_owned`,
  `execute_owned_raw_cell_object_for_slot_owned`, and `execute_closure_raw_cell_object_for_slot_owned` materialize owned
  values for loads and edge arguments.
- Expression execution: `execute_expr_owned` dispatches supported instruction kinds; `execute_binop_owned`,
  `execute_unary_op_owned`, `execute_getattr_owned`, `execute_getitem_owned`, `execute_setattr_owned`,
  `execute_setitem_owned`, `execute_delitem_owned`, `execute_make_cell_owned`, `execute_cell_ref_owned`,
  `execute_callee_function_id_owned`, `execute_call_owned`, and `execute_call_direct_owned` implement individual
  expression families.
- Calls and kwargs: `execute_call_parts_owned` evaluates positional/starred/keyword args, builds tuple/dict call objects,
  merges kwargs, formats duplicate-key errors, and invokes CPython call APIs.
- Terminators and raise/delete/store: `execute_raise_term_owned`, `execute_del_owned`, `execute_local_del_owned`,
  `execute_cell_del_owned`, `execute_global_del_owned`, `execute_store_owned`, `execute_local_store_owned`,
  `execute_cell_store_owned`, `execute_global_store_owned`, and `execute_return_global` implement terminator or statement
  behavior for resumed execution.
- Helper routines: `execute_abrupt_kind_arg_owned`, `owned_none`, `release_owned_values`, `set_raise_exception_owned`,
  `callable_soac_function_id`, `merge_kwargs_or_format_error`, `execute_unary_op_kind_owned`,
  `execute_binop_kind_owned`, `execute_runtime_name_deopt`, and `set_deopt_unbound_local_error` wrap repeated CPython
  semantics and error construction.

## Context Read

- `crates/soac_jit/src/jit/mod.rs`: defines `RuntimeJitDeoptInvocation`, deopt records/cursors/locals, data layout, and
  `abrupt_kind_tag`.
- `crates/soac_jit/src/jit/specialized_helpers.rs`: provides cell/global helper exports reused by the interpreter.
- `crate::module_constants`: loads runtime-name constants for deopt.
- `soac_blockpy::block_py`: instruction, term, call-argument, name-location, and operator datatypes interpreted here.
