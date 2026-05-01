---
title: "Lazy Per-Function Lowering"
---

# Lazy Per-Function Lowering

## Article Spark

V8 uses lazy parsing: discover function boundaries early, but defer full AST construction and
bytecode generation for functions that may never execute.

## SOAC Question

Can SOAC load a module with a lightweight function plan, then lower/prepare individual functions
only when they are called or known hot?

## Concrete Experiment

- Extend the module plan to preserve function source ranges and parent scope summaries.
- At import, lower `_dp_module_init` and function/object creation metadata eagerly, but defer full
  BlockPy/BB/codegen preparation for normal function bodies.
- On first call, parse/lower the function body, patch the module's shared function table, and enter
  the existing lazy JIT compile path.
- Combine with the file-backed BlockPy cache: cache each function artifact independently rather than
  requiring a whole-module artifact.

## Success Signal

- Importing a large module with many uncalled functions spends less time in SOAC parsing/lowering.
- First call still gets correct closure/global/type-param behavior.
- Cached per-function artifacts can be loaded without recovering one global `FunctionNameGen` state
  for the entire module.

## Risks

- Python's class bodies, decorators, defaults, annotations, type params, imports, and closures all
  have definition-time effects. Only function bodies should be deferred.
