# Compile Runtime Stubs From A Small IR

## Article Spark

V8 uses CodeStubAssembler as a cross-platform DSL for bytecode handlers and builtin stubs. The
stubs are close to runtime/VM internals but still share compiler backend machinery.

## SOAC Question

Should SOAC express more hot runtime helpers in a small typed IR and compile them with the same
backend/inlining path as user JIT code?

## Concrete Experiment

- Identify the current top runtime helpers in perf that are small and offset/guard heavy.
- Move one helper from C/Rust/PyO3 surface into a typed helper IR with explicit pointer/object/ref
  operations.
- Compile it to a standalone helper symbol and also make it importable/inlineable by user-function
  CLIF.
- Keep a reference Rust/C implementation for debug assertions or fallback until the IR version is
  well tested.

## Success Signal

- Helper logic used by globals/fields/calls has one source representation but can be emitted inline
  or as an out-of-line slow helper.
- Inlining threshold problems shrink because helpers can be split into "fast-path stub" and
  "slow-path call" at the IR level.

## Risks

- A second low-level IR may fight Cranelift CLIF unless it is deliberately tiny and VM-specific.
- Helpers that call arbitrary Python should stay out-of-line.
