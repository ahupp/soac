# Global JITModule Plan

## Goal

Move from the current "fresh `JITModule` per compiled runner" model to a
long-lived per-module JIT owner so transformed functions can:

- share stable CLIF symbols across one transformed Python module
- emit direct CLIF calls to other compiled SOAC functions
- eventually inline across function boundaries

The first success condition is not inlining. It is safe, explicit direct
calls between compiled SOAC functions with clear ownership, invalidation,
and recursion behavior.

## Design Constraints

- Preserve CPython-visible behavior first. If a direct-call path is not
  proven safe, it must fall back rather than guessing.
- Keep one owner for compiled code. The owner should be
  `SharedModuleState`, not a process-global singleton.
- Separate symbol identity from code generation. We need a stable callee
  symbol before we can compile arbitrary callers against it.
- Treat inlining as a later pass over a stable shared call graph, not as
  hidden behavior inside the compilation cache.

## Migration Steps

### 1. Introduce an explicit per-module JIT owner

Add a long-lived JIT owner under `SharedModuleState` that contains:

- one `JITModule`
- common CLIF/ISA/settings state used to declare and define functions
- a registry keyed by packed `FunctionId`

Suggested shape:

```rust
struct SharedJitModule {
    module: JITModule,
    functions: HashMap<FunctionId, CompiledFunctionState>,
}

enum CompiledFunctionState {
    Declared { func_id: cranelift_module::FuncId },
    Compiling { func_id: cranelift_module::FuncId },
    Ready {
        func_id: cranelift_module::FuncId,
        entry_ptr: *const u8,
        generation: u32,
    },
}
```

This step should not change call lowering yet. It only moves symbol and
code ownership into one place.

### 2. Split symbol declaration from body compilation

Add an early declaration pass over transformed functions so every
`FunctionId` gets one stable CLIF `FuncId` before any function body is
compiled.

That gives the first hard invariant:

- every transformed SOAC function has exactly one declared CLIF symbol in
  its owning shared module

This is the prerequisite for safe recursion and mutual recursion without
raw-pointer patching.

### 3. Change `CallDirect` to use CLIF direct calls

Replace the current immediate-pointer `call_indirect` path with:

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

### 4. Keep compilation lazy, but make symbols stable

Keep demand-driven body compilation:

- declaration may happen eagerly for all functions in the module
- code generation happens on first need
- once compiled, the symbol remains module-owned and stable

Caller `A` can force body compilation of callee `B`, but must not need to
invent a new symbol identity for `B`.

### 5. Add explicit generation/versioning

A shared module needs an invalidation story before it can safely replace
compiled code.

Initial safe policy:

- compile once per transformed module lifetime
- do not replace compiled bodies in place yet
- if invalidation becomes necessary, compile a new generation under an
  explicitly managed policy rather than silently reusing stale callers

The point of this step is to make "what code is current?" explicit before
we try aggressive re-specialization.

### 6. Move direct-call dependency tracking into the shared registry

The current `CallDirect` work already needs to know when a target is:

- declared but not compiled
- compiling recursively
- ready for direct call

Track direct-call dependencies explicitly:

- caller `FunctionId`
- callee `FunctionId`
- edge status: emitted direct, deferred, or forced to fallback

That prepares the system for fixed-point recompilation later without
inventing a second cache beside the shared module.

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

## Suggested First Implementation Slice

- Build `SharedJitModule` under `SharedModuleState`
- Eagerly declare every transformed function by `FunctionId`
- Teach `CallDirect` lowering to use declared `FuncId` plus CLIF `call`
- Keep current specialization policy and generic fallback unchanged
- Add:
  - one recursive-call test
  - one mutually-recursive-call test
  - one direct-call rendering test showing imported `FuncRef` use instead
    of immediate code pointers

## Challenging Parts

### Invalidation and replacement

A shared module makes symbol lifetime easier and code replacement harder.
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
