# crates/soac_jit/src/jit/runtime_context.rs

## File Responsibilities

Defines ABI-visible runtime context layouts and offset constants used by generated Cranelift code. It bridges Rust-owned
module/function state to raw pointer layouts that codegen can address directly, especially function environment fields and
`PyThreadState.current_exception`.

## Datatypes

- `PyThreadStateCurrentExceptionPrefix`: CPython `PyThreadState` prefix mirror through `current_exception`; used only to
  compute `PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET`.
- `ModuleJitContext`: C-layout module context carrying a raw pointer to `SharedModuleState` and an owned globals object.
- `FunctionEnvPrefix`: C-layout prefix of the per-function environment: direct code pointer, default direct code pointer,
  deopt table pointer, and globals object.
- `PyFunctionJitExtraPrefix`: C-layout prefix of SOAC bytes attached to a CPython function: function environment pointer and
  function id.
- `ModuleRuntimeContext`: Rust owner for `ModuleJitContext`, the `CompileSession`, and the `SharedModuleState` owner.
- `FUNCTION_ENV_*_OFFSET`: codegen offsets into `FunctionEnvPrefix`.
- `PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET`: codegen offset for loading the function environment from a function object's
  SOAC extra bytes.
- `PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET`: codegen offset for direct current-exception loads/stores.

## Functions

- `decref_if_non_null`: decrefs a raw Python object pointer when present.
- `ModuleRuntimeContext::drop`: releases the owned globals reference and poisons raw pointers in `mod_ctx` on teardown.

## Context Read

- `crates/soac_jit/src/jit/mod.rs`: uses these offsets and context types while generating direct calls, exception checks, and
  runtime state.
- `crate::module_type::SharedModuleState`: module-level state referenced by `ModuleJitContext`.
- `crate::session::CompileSession`: process/session state owned by `ModuleRuntimeContext`.
