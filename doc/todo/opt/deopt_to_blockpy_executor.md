---
title: "Real Deoptimization To A Generic Executor"
---

# Real Deoptimization To A Generic Executor

## Article Spark

V8 can speculate aggressively because deoptimization reconstructs an interpreter frame and resumes
execution at the matching bytecode offset. Failed speculation is a performance event, not an
incorrect result.

## SOAC Question

Should SOAC support "bail out of this JIT frame and resume the generic transformed executor" instead
of wiring every guard miss to a local helper/fallback expression?

## Concrete Experiment

- Define a deopt point table for each JIT function: BB label, instruction id, live locals, live
  closure cells, stack/temporary values, current exception state, and pending control-flow target.
- Implement one guard shape that calls a runtime deopt helper instead of a slow-path helper.
- Have the helper materialize a generic BlockPy/BB frame and resume the function at the deopt point.
- Start with a leaf function and a guard that has no Python-visible side effects before the bailout.

## Success Signal

- One direct field/global/call specialization can be emitted as "guard + direct path + deopt" with
  no in-function slow-path block.
- CLIF for a guard miss site shrinks versus the current local fallback lowering.
- Correctness tests demonstrate that side effects before the deopt point are not repeated.

## Risks

- CPython-visible frame, traceback, exception, generator, and refcount behavior make frame
  reconstruction hard.
- Local slow paths may remain the pragmatic answer for many operations.
