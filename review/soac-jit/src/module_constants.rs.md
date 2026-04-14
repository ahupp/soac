# soac-jit/src/module_constants.rs

## File Responsibilities

Collects constants required by codegen, builds their live Python object table, and resolves runtime-name constants from
`soac.runtime`/builtins. It also contains the bootstrap version of enough `soac.runtime` helpers to build constants while the
runtime module itself is being loaded, plus support for experimental static compact `PyLong` objects.

## Datatypes

- `ALWAYS_REQUIRED_UNICODE_CONSTANTS`, `ALWAYS_REQUIRED_RUNTIME_NAME_CONSTANTS`,
  `SOAC_RUNTIME_BOOTSTRAP_HELPER_NAMES`: prelude constants needed by codegen and runtime bootstrap.
- `ModuleConstantId`: index into a module's codegen constant table.
- `ModuleConstantValue`: interned constant payload kind: unicode, bytes, int, big int, float bits, or runtime name.
- `RuntimeNameConstantMode`: whether runtime names are imported from `soac.runtime` or built through the bootstrap module.
- `StaticPyObjectTemplate`: template for static Python object construction; currently compact one-digit positive `PyLong`.
- `RawPyLongDigit`, `RawPyLongValue`, `RawPyLongObject`, `RAW_PYLONG_*`: CPython layout mirrors and constants for compact
  `PyLong` construction.
- `ModuleCodegenConstants`: ordered constant pool plus de-duplication map.
- `ModuleConstantCollector`: visitor that scans lowered functions and terms for constants needed by generated code.

## Functions

- `StaticPyObjectTemplate::for_int`: selects values eligible for static compact `PyLong` construction.
- `StaticPyObjectTemplate::build_python_constant`: constructs the Python object for a template.
- `StaticPyObjectTemplate::compact_pylong_lv_tag`: computes the compact-long tag bits.
- `ModuleCodegenConstants::collect_from_module`, `collect_from_runtime_module`,
  `collect_from_module_with_runtime_prelude`, `collect_from_functions`: build constant pools from modules/functions.
- `ModuleCodegenConstants::build_python_constants`, `build_python_constants_for_soac_runtime`,
  `build_python_constants_with_runtime_names`: materialize constants as Python objects and make them immortal.
- `ModuleCodegenConstants` query methods: `len`, `require_*_constant_id`, `constant_bytes_value`,
  `constant_string_bytes_value`, `constant_string_value`, `constant_u64_value`, `constant_i64_value`, `constant_is_int`,
  `constant_runtime_name_value`.
- `ModuleCodegenConstants::lookup_id`, `push_explicit_constant_expr`, `intern`, `intern_unicode_bytes`,
  `intern_runtime_name_bytes`, `intern_int`: internal constant interning and explicit constant handling.
- `build_static_compact_pylong_i64`: allocates and initializes a compact CPython long object directly.
- `build_unicode_constant`: decodes UTF-8 with surrogatepass and interns the resulting unicode object.
- `build_runtime_name_constant`: resolves a runtime-name constant either through import lookup or bootstrap mode.
- `build_soac_runtime_bootstrap_runtime_name`: builds runtime-name constants while `soac.runtime` is being bootstrapped.
- `build_soac_runtime_bootstrap_module`: creates an in-memory Python module containing the subset of `soac.runtime` needed for
  bootstrap constants.
- `mark_constants_immortal`: calls CPython's immortal-object API for every module constant.
- `raise_name_error_for_missing_name`: formats and raises CPython-like `NameError` for missing runtime names.
- `load_runtime_name_owned`: resolves a name from `soac.runtime`, then builtins, returning an owned Python object or setting an
  exception.
- `should_include_in_locals_snapshot`: filters internal names from locals snapshots.
- `ModuleConstantCollector::collect_function`, `collect_stmt`, `collect_term`, `collect_block_args`, `collect_expr`: traverse
  lowered codegen IR to intern constants needed by locals, globals, attributes, deleted-name helpers, calls, and abrupt-kind
  transport.
- `ModuleConstantCollector::deleted_name_arg_bytes`: recognizes `load_deleted_name` calls and extracts their name argument.
- `ModuleConstantCollector::string_constant_bytes_for_specialized_codegen`: recovers string bytes from constants or `str(...)`
  wrappers used by specialized codegen.
- `Visit::visit_instr` for `ModuleConstantCollector`: delegates visitor traversal into `collect_expr`.
- `helper_name_for_codegen_expr`: recovers helper/runtime names from global, runtime-name, or constant loads.
- `abrupt_kind_tag`: maps abrupt-control-flow kinds to integer tags used in generated state.

## Context Read

- `soac-jit/src/jit/mod.rs`
- `soac-jit/src/module_type.rs`
- `soac_py/src/soac/runtime.py`
- `soac-blockpy/src/block_py.rs`

