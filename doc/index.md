# SOAC Documentation

This site collects design notes, pipeline walkthroughs, optimization plans, and
runtime reference material for SOAC.

## Start Here

- [Module Lifecycle](MODULE_LIFECYCLE.md) walks through the module dataflow and
  crate dependency graph from lowering through optimization and codegen.
- [Optimization](OPTIMIZATION.md) summarizes the optimization architecture.
- [Specialization](SPECIALIZATION.md) documents profile input, emitted
  specialization shapes, limitations, and soundness boundaries.
- [Special Names](SPECIAL_NAMES.md) inventories compiler/runtime-reserved names
  and documents where generated-name prefix checks are still part of a narrow
  implementation contract.
- [Runtime Functions](RUNTIME_FUNCTIONS.md) inventories runtime helper
  functions used by generated code.
- [Performance Log](PERF_LOG.md) records finalized performance changes.

## Planning Notes

The planning section contains active and historical design notes under
[`todo/`](todo/TODO.md). These are useful for understanding intended
architecture even when the implementation has moved ahead of a note.
