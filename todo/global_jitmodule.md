# Global JITModule Plan

## Goal

Move from the current "fresh `JITModule` per compiled runner" model to a
long-lived process JIT owner rooted in `CompileSession` so transformed
functions can:

- share stable CLIF symbols across transformed Python modules
- emit direct CLIF calls to other compiled SOAC functions
- eventually inline across function boundaries

The first success condition is not inlining. It is safe, explicit direct
calls between compiled SOAC functions with clear ownership, invalidation,
and recursion behavior.

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
- Treat inlining as a later pass over a stable shared call graph, not as
  hidden behavior inside the compilation cache.

## Current Status

- `CompileSession` owns a lazily-created `ProcessJitEngine`.
- `CompileSession` also owns the retained `SharedModuleState` registry used
  for cross-module `FunctionId` lookup.
- `ProcessJitState` owns one Cranelift `JITModule`, the direct-function
  declaration/ready registry, and shared vectorcall trampolines by arity.
- Direct function compilation walks reachable `CallDirect` edges and profiled
  call-target edges, predeclares the batch, defines bodies, finalizes once,
  and publishes `CompiledFunctionHandle`s.
- `FunctionEnv.direct_code_ptr` remains the runtime lazy-call fallback for
  direct edges that cannot use a predeclared CLIF symbol in the current batch.
- Vectorcall trampolines are process-JIT functions reused by arity.
- Codegen emits `soac_jit_direct_edges` tracing summaries for direct-edge
  decisions: CLIF direct calls, `FunctionEnv.direct_code_ptr` indirect calls,
  and generic-fallback reasons.

## Migration Steps

### 1. Keep compiled-code ownership in `CompileSession`

Status: implemented for production direct function bodies and vectorcall
trampolines.

Remaining cleanup:

- keep render/debug-only paths clearly marked as standalone or thread an
  explicit `CompileSession` through them when they need runtime state
- avoid adding new direct uses of `CompileSession::process()` below the
  runtime entry boundary

### 2. Split symbol declaration from body compilation

Status: implemented inside each process-JIT compile batch. The batch
collector finds reachable direct targets, declares all functions first, and
then defines bodies.

Implemented refinement:

- compile-time tracing now explains why an edge used a CLIF direct call,
  `FunctionEnv.direct_code_ptr`, or the generic Python call fallback

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
temporary fallback is `FunctionEnv.direct_code_ptr` plus `call_indirect`.
That keeps lazy compilation working while the process-JIT graph is still
being expanded.

### 4. Keep compilation lazy, but make batch symbols stable

Keep demand-driven body compilation:

- declaration happens eagerly for all functions in the current process-JIT
  batch
- code generation happens on first need
- once compiled, the symbol remains session-owned and stable

Caller `A` can force body compilation of callee `B`, but must not need to
invent a new symbol identity for `B`.

### 5. Add explicit generation/versioning

A process JIT needs an invalidation story before it can safely replace
compiled code.

Initial safe policy:

- compile once per transformed function shape in a `CompileSession`
- do not replace compiled bodies in place yet
- if invalidation becomes necessary, compile a new generation under an
  explicitly managed policy rather than silently reusing stale callers

The point of this step is to make "what code is current?" explicit before
we try aggressive re-specialization.

### 6. Move direct-call dependency tracking into the shared registry

The current `CallDirect` work already needs to explain when a target is:

- declared but not compiled
- compiling recursively
- ready for direct call

Track direct-call dependencies explicitly:

- caller `FunctionId`
- callee `FunctionId`
- edge status: emitted direct, deferred, or forced to fallback

That prepares the system for fixed-point recompilation later without
inventing a second cache beside the process JIT.

### 7. Add validation for cross-function call compatibility

Before arbitrary direct calls are enabled, validate that caller and callee
agree on the lowered call shape.

The first checks should be:

- positional argument arity matches
- bound receiver cases are modeled explicitly
- keyword cases are either supported or explicitly rejected
- result shape matches the generic `Call` contract at that site

This should live with the `Instr` validation work so bad direct-call
rewrites fail before CLIF lowering.

### 8. Add observability for unresolved edges and fallback reasons

Before inlining, expose:

- direct-call hits vs generic fallback
- edges blocked by missing compiled state
- edges blocked by signature mismatch
- recursive SCCs with deferred direct edges

This can live in the existing binary counter dump or a sibling JIT-state
debug dump. The goal is to explain the optimization behavior, not just to
make it faster.

### 9. Add a small explicit inliner only after direct calls are stable

Do not start with arbitrary inlining.

First slice:

- leaf functions only
- small body-size threshold
- no exception edges
- no unsupported control-flow or environment-sensitive shapes

This should operate over a stable shared call graph, not through ad hoc
runtime rewriting.

### 10. Roll out in stages behind flags

Suggested sequence:

1. shared declaration registry only
2. direct CLIF calls for `CallDirect`
3. lazy body compilation against shared symbols
4. dependency tracking for unresolved direct edges
5. optional fixed-point recompilation
6. optional inlining

Each stage should be individually switchable so regressions can be
isolated quickly.

## Next Implementation Slices

1. Finish explicit-session cleanup.
   - Keep `SharedModuleState` free of hidden process-singleton lookups.
   - Thread the current `Arc<CompileSession>` into any remaining production
     compile or direct-target lookup path.

2. Tighten recursion and cross-module tests.
   - Recursive direct call in one module.
   - Mutually-recursive direct calls in one module.
   - Cross-module direct call where the callee is found through the
     `CompileSession` retained-state registry.

3. Remove the temporary indirect direct-call path once the batch collector
   reliably covers all supported direct edges.
   - After that, supported direct edges should be CLIF `call`; unsupported
     edges should go through the generic Python call fallback.

4. Benchmark before inlining.
   - Measure direct-call heavy workloads before changing the inliner.
   - Record any finalized performance result in `docs/CODEX_OPT_LOG.md`.

## Challenging Parts

### Invalidation and replacement

A process JIT makes symbol lifetime easier and code replacement harder.
This is the largest architectural risk.

### Recursive compilation

Declaration must be separate from body compilation or recursive direct
calls will deadlock or spuriously fall back.

### Bound-method direct calls

To remove the remaining bound-method overhead, direct-call guards need to
cover:

- safe callable shape
- implicit receiver handling
- instance shadowing / descriptor lookup assumptions where we bypass
  `PyMethod` construction

### Inlining safety

Inlining needs a stable call graph and explicit safety checks. It should
come only after direct-call correctness, validation, and observability are
already in place.
