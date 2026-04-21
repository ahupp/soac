# crates/soac_jit/src/session.rs

## File Responsibilities

Owns process-scoped compilation state. `CompileSession` allocates module ids, retains shared module states so direct
cross-module lookup remains valid, and lazily owns the process-global Cranelift JIT engine. It is the top-level state object
passed through runtime/JIT code instead of using scattered globals.

## Datatypes

- `NEXT_COMPILE_SESSION_ID`: global atomic id issuer.
- `PROCESS_COMPILE_SESSION`: process-wide lazy `Arc<CompileSession>`.
- `CompileSessionId`: small numeric id for a compile session.
- `CompileSession`: process/session owner for ids, retained module states, and process JIT.
- `SharedModuleStateRegistry`: internal retained `Arc<SharedModuleState>` list plus module-id index.

## Functions

- `CompileSessionId::as_u32`: exposes the numeric session id.
- `allocate_compile_session_id`: returns a fresh monotonically increasing session id.
- `SharedModuleStateRegistry::retain`: retains a module state and indexes it by module id.
- `SharedModuleStateRegistry::for_function_id`: finds the retained module state for a function id's module id.
- `SharedModuleStateRegistry::retained_len`: test-only retained-state count.
- `CompileSession::new`: creates an empty session with a fresh id and module-id counter.
- `CompileSession::process`: returns the process-global session.
- `CompileSession::id`: returns this session's id.
- `CompileSession::module_name_gen`: allocates a fresh `ModuleNameGen`/module id.
- `CompileSession::process_jit`: lazily constructs or returns the process JIT engine.
- `CompileSession::retain_shared_module_state`: adds a transformed module's shared state to the session registry.
- `CompileSession::shared_module_state_for_function_id`: finds retained module state by function id.
- `CompileSession::lookup_shared_function`: finds and clones a lowered function from the retained module registry.
- `CompileSession::retained_shared_module_state_count`: test-only registry size.
- `Default for CompileSession`: delegates to `new`.
- `Debug for CompileSession`: prints only session id and leaves internals non-exhaustive.

## Context Read

- `crates/soac_jit/src/module_type.rs`
- `crates/soac_jit/src/jit/mod.rs`
- `crates/soac_jit/src/jit/runtime_context.rs`

