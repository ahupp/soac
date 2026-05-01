---
title: "Escape Analysis For Temporary Python Objects"
---

# Escape Analysis For Temporary Python Objects

## Article Spark

V8's top optimizing tier can remove allocations when escape analysis proves an object does not leave
the optimized function.

## SOAC Question

Which transformed-runtime temporary Python objects can be proven not to escape across Python-visible
boundaries?

## Concrete Experiment

- Add an analysis over Codegen BlockPy / BB values that marks whether a value can reach:
  - return / yield / raise
  - a Python call
  - object attribute/item store
  - closure cell or module global
  - a container object
- Start with compiler-generated tuples for call packing, temporary exception payloads, and helper
  result pairs.
- For a proven-local tuple or pair, replace allocation with raw locals/registers plus specialized
  consumers.

## Success Signal

- Perf shows fewer PyTuple/PyObject allocation and decref hotspots on pystone.
- A focused transformed-runtime test still observes correct object identity for user-created
  objects, while compiler-generated temporaries disappear from the hot path.

## Risks

- In Python, many operations can re-enter Python or expose values through traceback/frames.
- The first implementation should target compiler-generated temps, not arbitrary user literals.
