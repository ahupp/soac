---
title: "Global JITModule Plan"
---

# Global JITModule Plan

## Goal

Move from the current "fresh `JITModule` per compiled runner" model to a
long-lived process JIT owner rooted in `CompileSession` so transformed
functions can:

- share stable CLIF symbols across transformed Python modules
- emit direct CLIF calls to other compiled SOAC functions

The first success condition was safe, explicit direct calls between compiled
SOAC functions with clear ownership and recursion behavior.

## Design Constraints

- Preserve CPython-visible behavior first. If a direct-call path is not
  proven safe, it must fall back rather than guessing.
- Keep one owner for compiled code. `CompileSession` owns the
  `ProcessJitEngine`; `SharedModuleState` owns transformed module data and
  is retained by the session so cross-module lookups have a stable root.
- Pass `Arc<CompileSession>` explicitly through runtime and codegen paths.
  Avoid hidden `CompileSession::process()` lookups inside `SharedModuleState`
  helpers or other code that should be scoped to the active session.
- Separate symbol identity from body code generation. We need a stable callee
  symbol before we can compile arbitrary callers against it, especially for
  recursive and mutually-recursive functions.

## Current Status

- `CompileSession` owns a lazily-created `ProcessJitEngine`.
- `CompileSession` also owns the retained `SharedModuleState` registry used
  for cross-module `FunctionId` lookup.
- `ProcessJitState` owns one Cranelift `JITModule`, the direct-function
  declaration/ready registry, and shared vectorcall trampolines by arity.
- Direct function compilation walks reachable `CallDirect` edges and profiled
  call-target edges, predeclares the batch, defines bodies, finalizes once,
  and publishes `CompiledFunctionHandle`s.
- `FunctionEnv.direct_code_ptr` remains the vectorcall entry pointer, but
  SOAC-to-SOAC direct-call lowering no longer emits a `call_indirect` through
  it. Supported direct edges must have a predeclared process-JIT symbol;
  unsupported edges use the generic Python call fallback.
- Vectorcall trampolines are process-JIT functions reused by arity.
- Codegen emits `soac_jit_direct_edges` tracing summaries for direct-edge
  decisions: CLIF direct calls and generic-fallback reasons.
- Focused tests cover recursive batch collection, mutually-recursive batch
  compilation, and cross-module batch collection through the retained
  `CompileSession` shared-state registry.

## Migration Steps

### 1. Keep compiled-code ownership in `CompileSession`

Status: implemented for production direct function bodies, vectorcall
trampolines, and render/debug JIT builder paths. Production runtime entry
points acquire the process session, lower JIT codegen takes an explicit
`CompileSession`, and standalone render wrappers create a fresh non-process
session so they cannot observe or mutate production process-JIT state.

Guardrail:

- avoid adding new direct uses of `CompileSession::process()` below the
  runtime entry boundary

### 2. Split symbol declaration from body compilation

Status: implemented inside each process-JIT compile batch. The batch
collector finds reachable direct targets, declares all functions first, and
then defines bodies.

Implemented refinement:

- compile-time tracing now explains why an edge used a CLIF direct call or the
  generic Python call fallback

The hard invariant should become:

- every transformed SOAC function compiled in a `CompileSession` has at most
  one declared CLIF symbol for its current validated shape

### 3. Change `CallDirect` to use CLIF direct calls

Status: mostly implemented for edges that are present in the process-JIT
predeclared batch.

The intended lowering remains:

- look up the callee's declared `FuncId`
- import it into the caller IR with `declare_func_in_func`
- emit a normal CLIF `call`

Sketch:

```rust
let callee_func_id = shared_jit.lookup_declared_func_id(target_function_id)?;
let callee_ref = jit_module.declare_func_in_func(callee_func_id, fb.func);
let call = fb.ins().call(callee_ref, &args);
```

Keep generic `Call(...)` fallback in place until direct-call correctness is
proven.

When a direct edge is not present in the current predeclared batch, the
fallback is the generic Python call path. This avoids carrying a second
SOAC-to-SOAC call convention beside the process-JIT symbol path.

### 4. Keep compilation lazy, but make batch symbols stable

Keep demand-driven body compilation:

- declaration happens eagerly for all functions in the current process-JIT
  batch
- code generation happens on first need
- once compiled, the symbol remains session-owned and stable

Caller `A` can force body compilation of callee `B`, but must not need to
invent a new symbol identity for `B`.

### 5. Rollout status

Completed sequence:

1. shared declaration registry
2. direct CLIF calls for `CallDirect`
3. lazy body compilation against shared symbols

## Completion Notes

- Declaration is separate from body compilation, so recursive direct calls do
  not need to invent duplicate symbols.
- A single process-owned `JITModule` now holds direct function bodies and
  shared vectorcall trampolines.
- Unsupported direct edges still fall back to the generic Python call path.
