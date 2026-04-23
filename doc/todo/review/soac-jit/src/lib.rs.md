# crates/soac_jit/src/lib.rs

## File Responsibilities

Crate root and Python-function integration layer for SOAC's JIT extension. It exposes submodules, embeds generated runtime
CLIF, manages CPython metadata attached to `PyFunction`/heap types, creates per-function JIT environments, binds vectorcall
arguments for direct entries, lazily compiles CLIF function bodies/trampolines, and registers owner-type relationships used by
constructor-local specialization metadata.

## Datatypes

- Public modules: `config`, `counter`, `counter_dump`, `module_constants`, `module_type`, `session`; private JIT internals are
  re-exported from `jit`.
- `PyFunctionWatchEvent` / `PyFunctionWatchCallback`: CPython function watcher ABI aliases.
- `PY_FUNCTION_EVENT_*`: CPython function watcher event codes handled by this layer.
- `FunctionEnvAbiHeader`: ABI-prefix visible to generated code, containing direct entry pointers, deopt table pointer, and
  globals object.
- `FunctionEnv`: Rust owner for the ABI allocation and variable-sized runtime object array containing defaults, closures, and
  other function state.
- `PyFunctionJitExtra`: SOAC metadata attached to a Python function; holds function id, function environment, argument binding
  plan, compile session, module state, vectorcall entry, and previous vectorcall.
- `DirectArgParamBinding`: parameter metadata needed for vectorcall-to-direct argument binding.
- `DirectArgBindingPlan`: precomputed binding plan for positional, keyword, `*args`, `**kwargs`, and default slots.
- `RegisteredFunctionOwnerTypes`: weakrefs from one function to owner types that currently define it.
- `FunctionOwnerTypeRegistry`: process-wide function watcher plus weakref registry for constructor owner metadata.
- `ConstructorOwnerType`: exact owner type plus `__init__` function/version data.

## Functions

- Test helpers `python_runtime_test_lock`, `test_repo_root`, `vendored_python_home_for_test`,
  `vendored_python_build_lib_dir_for_test`, `test_extension_staging_dir`, `test_python_import_paths`,
  `initialize_test_python`, `run_test_in_isolated_process_if_needed`: configure isolated Rust tests that need vendored CPython
  and the built extension.
- `panic_payload_to_string`: formats panic payloads for Python-facing error messages.
- `function_owner_type_watcher_callback`: CPython watcher that restores vectorcall on code mutation, refreshes defaults/closure
  runtime objects, invalidates owner types on function changes, and releases weakrefs on destroy.
- `function_owner_type_registry`: lazily installs the CPython function watcher and returns the registry.
- `set_runtime_error`, `set_type_error`, `binding_type_error`: set Python exceptions and return `Err(())`.
- `DirectArgBindingPlan::from_function`: builds the argument-binding plan from lowered function parameters and runtime data
  layout.
- `DirectArgBindingPlan::param_count`, `positional_capacity`, `param_index`: query binding-plan shape.
- `FunctionEnv::runtime_objects_offset`, `allocation_layout`, `new`: allocate and initialize the ABI header plus runtime object
  array, owning references.
- `FunctionEnv` accessors/mutators: expose the ABI pointer, header fields, globals, direct entry pointers, deopt table pointer,
  and runtime-object slots.
- `FunctionEnv::replace_runtime_objects`: atomically replaces stored default/closure object references after Python function
  metadata changes.
- `FunctionEnv::drop`: decrefs stored runtime objects and globals, then deallocates the ABI block.
- `PyFunctionJitExtra::function`, `runtime_data_layout`, `refresh_runtime_objects_after_function_update`: recover the lowered
  function and refresh runtime object slots after `__defaults__`/`__kwdefaults__` changes.
- `free_clif_function_data`: destructor passed to CPython SOAC metadata to drop `PyFunctionJitExtra`.
- `py_unicode_utf8_str`: validates and borrows a keyword argument name as UTF-8.
- `collect_function_runtime_objects`: collects positional defaults, keyword-only defaults, and closure cells into the
  function-env runtime object array with owned references.
- `clone_module_runtime_context`: clones a module runtime context and increfs globals for another function registration.
- `build_module_runtime_context_for_module`: obtains transformed module state and globals from a Python module.
- `make_clif_function_data`: creates `PyFunctionJitExtra` for a Python function and lowered function id.
- `py_function_jit_extra`: validates a Python function and returns attached SOAC metadata.
- `registered_clif_function_id`, `registered_clif_type_function_id`: read SOAC function ids from CPython function/type
  metadata.
- Owner-type registration helpers: `register_owner_type_for_function`, `incref_weakref_snapshot`, `resolve_weakref_target`,
  `lookup_exact_owner_types_for_function_object`, `lookup_exact_owner_types_for_constructor_object`,
  `lookup_exact_owner_types_for_constructor`, `type_is_defined_in_module`, `register_owner_types_from_type`,
  `register_function_owner_type_value`, `register_function_owner_type_indexed_key`,
  `register_function_owner_types_for_globals`, `register_function_owner_types_for_module`,
  `register_function_owner_types_for_module_keys`.
- `ensure_clif_vectorcall_compiled`: lazily compiles or finds the direct function body, installs direct entry/deopt pointers in
  `FunctionEnv`, compiles the shared vectorcall trampoline, and patches the Python function vectorcall.
- Argument cleanup/binding helpers: `cleanup_state_values`, `cleanup_output_args`, `initialize_output_args`,
  `output_arg_is_assigned`, `write_output_arg_from_borrowed`, `write_output_arg_from_owned`,
  `bind_function_args_to_output`.
- `bind_direct_args_from_vectorcall`: C ABI trampoline helper used by generated vectorcall code to bind vectorcall args into
  direct-entry argument buffers.
- `vectorcall_compile_function_env`: C ABI helper used by vectorcall trampolines to force compilation and return the
  `FunctionEnv` pointer.
- `register_clif_vectorcall`: attaches SOAC metadata to a Python function and installs the shared vectorcall trampoline.
- `compile_clif_vectorcall`: eagerly compiles the function body/trampoline for an already-registered Python function.

## Context Read

- `crates/soac_jit/src/jit/mod.rs`
- `crates/soac_jit/src/jit/runtime_context.rs`
- `crates/soac_jit/src/jit/direct_abi.rs`
- `crates/soac_jit/src/module_type.rs`
- `soac-blockpy/src/block_py.rs`
