# soac-pyo3/src/jit_runtime.rs

## File Responsibilities

Python extension runtime bridge for transformed modules. It implements `_soac_ext.create_module`, `_soac_ext.exec_module`, `_soac_ext.make_function`, and type-layout profiling hooks; lowers module source to BlockPy; creates SOAC extension-module state; instantiates Python function objects; registers JIT vectorcall metadata; and logs module-load timings.

## Datatypes

- `SOAC_BUILD_IDENTITY`: build identity embedded into module-cache lookup paths.
- `SoacRuntimeBootstrapGlobals`: saved original `soac.runtime` helper globals restored after bootstrapping that module.
- `OriginalCodeMap`: map from SOAC `FunctionId` to original CPython code objects.
- `OriginalCodeByQualname`: queue of CPython code objects grouped by qualname to handle duplicate names.
- `TimedPhase`: named duration for create/exec/lowering phases.
- `PendingModuleLoadTiming`: create-module timing and module metadata retained until `exec_module` completes.
- `PENDING_MODULE_LOAD_TIMINGS`: process map from module object pointer to pending timing data.

## Functions

- `is_cell_object`: checks whether a raw Python object pointer is a `cell`.
- `import_dp_module`: imports `soac.runtime`.
- `install_soac_runtime_bootstrap_sentinel`: installs runtime singleton constants into `soac.runtime` during bootstrap.
- `SoacRuntimeBootstrapGlobals::restore`: restores bootstrap-overwritten helper globals.
- `install_soac_runtime_bootstrap_globals`: prepares `soac.runtime` to execute through its own transformed module init.
- `tuple_from_owned_objects`: builds a Python tuple by stealing references from owned PyO3 objects.
- Timing/log helpers: `time_phase`, `elapsed_us`, `source_hash_hex`, `pending_module_load_timings`, `pending_module_load_timing_key`, `store_pending_module_load_timing`, `take_pending_module_load_timing`, `trace_module_load_phase`, and `append_completed_module_load_log` collect and emit structured module-load timings.
- Cache/path helpers: `soac_repo_root` and `pre_optimization_module_cache_path` resolve the serialized BlockPy module cache path for current source/build/runtime-name mode.
- `module_spec_string_attr`: reads required string attributes from an importlib module spec.
- `compile_original_module_code`: compiles source with CPython to obtain original code objects.
- `collect_original_code_objects`: recursively collects nested CPython code objects by qualname.
- `is_synthetic_class_helper`: identifies generated class-namespace helper functions that do not map to source code objects.
- `original_code_lookup_key`: decides whether a lowered function should be matched to an original code object.
- `match_original_code_to_functions`: maps lowered functions to CPython code objects by qualname.
- `make_lazy_clif_entry`: creates a Python function object from an original or template code object.
- `register_clif_vectorcall_raw`: attaches SOAC JIT vectorcall metadata to a Python function.
- `maybe_eager_compile_clif_entry`: optionally compiles the function JIT entry immediately.
- `register_jit_vectorcall`: clones runtime context and registers/eager-compiles a function entry.
- Error-suppression helpers: `ignore_attr_or_type_error` and `ignore_attr_or_value_error` ignore expected attribute mutation failures while preserving unexpected errors.
- `update_function_metadata`: sets Python-visible function metadata such as name, qualname, docstring, code name, and annotations.
- `resolve_module_name` and `resolve_module_package`: read required module metadata from globals.
- `module_runtime_from_shared_state`: creates a `ModuleRuntimeContext` from compile session, shared state, and module globals.
- `lookup_module_init_function`: finds the lowered `_dp_module_init` function.
- Closure/default helpers: `build_capture_map`, `normalize_class_cell_capture`, `split_param_defaults`, `build_closure_shaped_entry`, and `apply_function_defaults` turn lowered captures/defaults into CPython function closure/default structures.
- `instantiate_bb_function`: constructs a Python callable for one lowered function, applies metadata/defaults, and registers JIT vectorcall unless preserving source runtime helpers.
- `function_kind_name`: converts lowered function kind to the runtime string protocol.
- `mark_coroutine_function`: adds asyncio coroutine marker metadata.
- `instantiate_closure_backed_entry`: chooses direct lazy entry vs closure-shaped function construction.
- `make_function`: Python-exposed factory for nested/lazy lowered functions by `FunctionId`.
- `create_module`: Python-exposed import hook create step; reads source, lowers BlockPy, caches metadata, creates a SOAC extension module, and stores pending timing data.
- `ensure_module_builtins`: installs `__builtins__` into module globals if absent.
- `exec_module`: Python-exposed import hook exec step; wraps `exec_module_inner` with timing/log emission.
- `exec_module_inner`: instantiates and calls the lowered module init function, handles `soac.runtime` bootstrap, and registers owner-type invalidation metadata.
- `profile_watch_type_key_layout`: Python-exposed hook to record split-key layout metadata for a type.
- `add_module_functions`: registers runtime bridge functions into the `_soac_ext` module.

## Context Read

- `soac-pyo3/src/lib.rs`
- `soac_py/src/soac/import_hook.py`
- `soac_py/src/soac/runtime.py`
- `soac_jit::module_type`
- `soac_jit::config`

