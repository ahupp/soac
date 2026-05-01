---
title: "JIT Tracebacks From Unwind Metadata"
---

# JIT Tracebacks From Unwind Metadata

## Goal

Produce CPython traceback objects for Python frames that executed as SOAC
direct-entry JIT code.

The native unwind data already emitted for perf is the right foundation for
finding active JIT callers, but it is not a Python traceback by itself.  A
Python traceback node still needs a `PyFrameObject`, a code object, a lasti /
line, and insertion into the current raised exception's `__traceback__`.

## Existing facts

- `crates/soac_jit/src/jit/jitdump.rs` serializes Cranelift `SystemVUnwindInfo` into
  `.eh_frame` bytes and writes a `PERF_JIT_CODE_UNWINDING_INFO` record before
  the matching `PERF_JIT_CODE_LOAD` record.
- That jitdump copy is for perf.  Runtime traceback recovery should not parse
  the jitdump file.
- `vendor/cpython/Python/traceback.c` implements `PyTraceBack_Here(frame)` by
  prepending a node to the current raised exception.
- `PyTraceBack_Here(frame)` derives `tb_lasti` from `frame->f_frame`.  If SOAC
  creates a placeholder frame without positioning its instruction pointer,
  traceback line numbers will be wrong.
- SOAC already compiles and stores original CPython code objects for normal
  user functions.  Those are the best `tb_frame.f_code` objects for tracebacks.
- BlockPy instructions carry `Meta.range`; source byte offsets can be lowered
  to traceback line / column metadata.

## Design principle

Attach traceback information while the relevant JIT frames are still on the
native stack.

If a direct-entry runner returns `NULL`, its native frame is gone.  The runtime
helper that recovers SOAC Python frames must run from compiled cleanup /
exception-dispatch code before returning through the direct-entry ABI.

## Runtime unwind registration

First audit whether the Cranelift `JITModule` memory manager already registers
JIT unwind data with the platform unwinder.

If it does not, keep a runtime copy of the serialized `.eh_frame` next to the
compiled function and register it with the process unwinder when the function is
finalized:

```text
compile direct runner
  -> Cranelift SystemVUnwindInfo
  -> serialize .eh_frame
  -> register with unwinder, e.g. __register_frame on Linux/glibc targets
  -> write jitdump copy for perf, as today
  -> keep registered bytes alive until CompiledSpecializedRunner drop
```

Deregister the frame data when the compiled handle is freed.  Keep the perf
jitdump path as a consumer of the same unwind bytes, not as the source of truth.

## Code registry

Add an in-process registry for finalized SOAC JIT code:

```text
JitCodeRegistry:
  code_start..code_end -> JitCodeTraceInfo

JitCodeTraceInfo:
  code_kind: DirectPython | VectorcallTrampoline | RuntimeStub
  function_id: FunctionId
  original_code: Py<PyAny>
  globals: Py<PyAny>
  qualname: String
  pc_statepoints: Vec<PcTraceStatepoint>
  leaf_cleanup_statepoints: Vec<TraceStatepoint>
```

Direct Python runners are traceback-visible.  Vectorcall trampolines and helper
stubs are native implementation details and should be skipped while walking.

Register direct runners after `finalize_definitions()` returns a stable code
pointer.  Deregister them from the compiled-handle `Drop`.

## Statepoint metadata

Introduce a compact `TraceStatepointId` for each instruction / terminator that
can set a Python exception or call Python code.

Each statepoint records:

```text
TraceStatepoint:
  function_id
  instr_id or BlockLabel
  source range
  lineno
  original-code bytecode offset, when known
  fallback traceback name / filename
```

Codegen should tag potentially-raising native operations with this statepoint:

- For calls out of a JIT frame, record the PC range around the call so a caller
  frame's return address maps to that callsite.
- For the leaf frame that is about to return `NULL`, pass the current leaf
  statepoint id to the traceback helper, because the current PC is usually in a
  shared cleanup block rather than at the original raising call.
- If Cranelift exposes source-location-to-machine-code maps after compile, use
  Cranelift `SourceLoc` for the PC table.  If that is not available, maintain
  explicit statepoint ids at call/branch lowering sites and patch the PC table
  from the finalized code-info API that is available.

## Traceback helper

Add a hot-path-minimal runtime helper called from compiled code only on
exception exits:

```text
soac_jit_attach_traceback_for_unhandled_exception(
    leaf_function_env,
    leaf_statepoint_id,
)
```

The helper should:

1. Fetch the current raised exception; if none exists, return.
2. Walk the native stack with a normal unwinder.
3. Resolve instruction pointers against `JitCodeRegistry`.
4. Keep traceback-visible `DirectPython` frames and skip SOAC trampolines /
   helper stubs.
5. Use `leaf_statepoint_id` for the innermost current frame; use return-address
   PC lookup for older active SOAC callers.
6. Stop at the vectorcall / CPython boundary for the current SOAC call unless a
   nested SOAC direct call is still active above it.
7. Compare the frames it plans to attach with the current traceback prefix, and
   do not add frames that were already attached by an inner SOAC cleanup.
8. Attach frames in inner-to-outer order, matching CPython unwinding behavior:
   each prepend makes the final traceback order outer-to-inner-to-existing.

## Materializing Python frame / traceback objects

Prefer a small vendored-CPython helper over calling Python-level constructors in
an exception path.

Recommended helper shape:

```text
_SoacTraceBack_Here(
    PyCodeObject *code,
    PyObject *globals,
    PyObject *locals_or_null,
    int lasti,
    int lineno,
)
```

The helper should allocate a `PyFrameObject` from the original code object,
position the frame when a bytecode offset is available, set an explicit line
fallback, create / prepend a traceback object, and restore the raised exception.

If directly constructing `PyTracebackObject` in a CPython helper is too invasive
for the first patch, use `PyFrame_New()` plus internal frame positioning before
calling `PyTraceBack_Here(frame)`.

## Exception-path integration

Phase 1: unhandled exceptions that leave the JIT through vectorcall.

- In every direct-runner cleanup block that returns `NULL`, call the traceback
  helper before decref cleanup if an exception is set.
- Use a per-raised-exception / traceback-prefix guard in the helper so a JIT
  caller that also hits cleanup does not duplicate frames already attached by an
  inner failed direct call.

Phase 2: exceptions caught inside the same JIT frame.

- Before jumping from a raising operation to a SOAC exception-dispatch block,
  attach only the current leaf frame.
- Do not unwind and attach outer JIT callers for an exception that is still
  being handled inside the current Python frame.

Phase 3: direct JIT-to-JIT calls.

- Ensure direct-call codegen has a statepoint around the indirect call.
- When the inner callee fails and attaches its leaf frame, the outer caller's
  unhandled cleanup should either attach only the missing outer frame or find
  that the full active JIT stack was already attached.

## Locals policy

Start with correct function name, filename, line, and code object.

Frame locals can initially be empty or a best-effort dict.  A later patch can
materialize stack-slot locals from `StackSlots` metadata at the exception
statepoint.  Do not expose bogus locals just to fill `f_locals`.

## Validation

- Add an integration test for a transformed function that raises and whose
  traceback contains the original file, original function name, and raising
  source line.
- Add a nested transformed-call test:
  `outer -> middle -> inner -> raise`, expecting the Python traceback order to
  be `outer, middle, inner`, with no SOAC trampoline frames.
- Add a mixed call test where JIT Python calls a CPython function that raises;
  the existing CPython traceback should remain at the tail.
- Add a caught-exception test after phase 2:
  inside `except Exception as exc`, `traceback.extract_tb(exc.__traceback__)`
  includes the raising SOAC frame.
- Keep the perf helper-frame tests: native perf helper visibility and Python
  traceback visibility are separate contracts.

## Open questions

- Does the current Cranelift `JITModule` register unwind info for native
  in-process unwinding, or is the current `.eh_frame` only exported to perf?
- Which Cranelift code-info API is available in this dependency set for mapping
  `SourceLoc` / callsites to machine-code offsets?
- Should traceback materialization live entirely in vendored CPython, or should
  vendored CPython expose exactly one `_SoacTraceBack_Here` helper and keep SOAC
  frame selection / PC mapping in Rust?
- How complete do locals need to be before enabling the feature by default?
