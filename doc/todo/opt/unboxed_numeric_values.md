---
title: "Unboxed Numeric Values"
---

# Unboxed Numeric Values

## Article Spark

V8 optimizing tiers choose representations: tagged small ints where necessary, raw Int32/Float64 in
registers where profitable, boxed heap numbers at object boundaries.

## SOAC Question

Can SOAC keep exact-int and float intermediates unboxed through straight-line arithmetic and loop
carried values, boxing only at Python-visible boundaries?

## Concrete Experiment

- Add shape feedback for numeric-producing instructions and compare sites, not just operator input
  sites.
- Add an internal value representation for "raw exact int64 with Python overflow checks pending" or
  "raw double".
- Lower short arithmetic chains to raw machine ops under exact-type guards.
- Box when a value is returned to Python, stored in an object/dict/cell/global, passed to unknown
  Python callable, or exposed through an exception/traceback/frame boundary.

## Success Signal

- Arithmetic-heavy transformed loops stop allocating intermediate PyLong objects on the hot path.
- Exact-int pystone operator specializations become raw add/sub/compare sequences, not just direct
  PyLong slot calls.
- Overflow and bool-subclass edge cases fall back correctly.

## Risks

- Python `int` has arbitrary precision and `bool` is an `int` subclass.
- Destructor timing is not the only issue; object identity can be observed whenever values cross a
  Python-visible boundary.
