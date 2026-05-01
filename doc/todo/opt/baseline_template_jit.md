---
title: "Baseline Template JIT Tier"
---

# Baseline Template JIT Tier

## Article Spark

V8's Sparkplug tier compiles bytecode to native code in one linear pass. It does not build an
optimizer IR; it emits instruction templates and calls builtins for complex operations. The value is
removing interpreter dispatch at a very low compile cost.

## SOAC Question

Can SOAC add a cheaper "baseline native" tier for cold and medium-hot functions, before paying for
the current full CLIF/JIT pipeline?

## Concrete Experiment

- Pick a narrow BlockPy/BB subset that is common in import-time and helper code: straight-line
  blocks, local/global load/store, constants, branches, calls through runtime helpers, return.
- Build a single-pass emitter that walks BB instructions and emits either:
  - direct machine code through a tiny assembler layer, or
  - very shallow Cranelift without specialization, inlining, counter wiring, or runtime CLIF
    imports beyond a fixed helper table.
- Use the same callable wrapper and function metadata as the current JIT path.
- Tier up from baseline to the current specialized JIT once the function is hot or once a
  specialization counter profile exists.

## Success Signal

- Compile latency per small function drops materially versus current lazy CLIF compilation.
- Pystone / integration warmup gets faster without lowering the specialized second-pass benchmark.
- Generated perf profile shows less time in compile/lazy-entry paths during first calls.

## Risks

- A new tier is a large maintenance surface unless it consumes the same finalized BB/BlockPy
  operation set.
- If current lazy CLIF compile cost is already amortized away in target workloads, this is not the
  next bottleneck.
