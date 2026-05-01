---
title: "Restore Lazy Annotation Support"
---

# Restore Lazy Annotation Support

## Goal

Restore module, class, and function annotation behavior in the normal SOAC lowering path.

Do not restore this by depending on original CPython bytecode or by making
`annotationlib` clone a SOAC-backed function via `types.FunctionType(annotate.__code__, ...)`.
Annotation thunks should be ordinary lowered SOAC functions whose public behavior matches
CPython's `__annotate__`, `__annotate_func__`, and `__annotations__` protocol.

CPython names to preserve:

- modules store `__annotate__` in the module dict.
- class namespace dictionaries store `__annotate_func__`; heap type descriptors expose it as
  `SomeClass.__annotate__`.
- functions store a callable in `PyFunctionObject.func_annotate`; Python exposes it as
  `some_function.__annotate__`.

## Current State

- `soac-blockpy/src/passes/ast_to_ast/rewrite_stmt/annotation.rs` collects module and class
  `AnnAssign` statements and generates a synthetic annotation helper.
- The same pass strips annotations found inside function bodies.
- Function signature annotations are not collected into a helper.
- `soac-blockpy/src/passes/ruff_to_blockpy/module_plan/mod.rs` always passes `None` as the
  `annotate_fn` argument when it materializes `__soac__.make_function(...)`.
- `crates/soac_pyo3/src/jit_runtime.rs` already has the runtime hook: `update_function_metadata(...)`
  sets `func.__annotate__` when the `annotate_fn` argument is not `None`.
- The generated module/class helper handles VALUE and STRING, but raises `NotImplementedError`
  for FORWARDREF. That sends CPython's `annotationlib.call_annotate_function(...)` down its
  fake-globals path.

## Why CPython's Fake-Globals Fallback Is Not The SOAC Path

CPython compiler-generated annotation functions primarily implement VALUE.

For STRING and FORWARDREF, `annotationlib` may construct a new function from the original
annotation function code object with replacement globals and closure cells. Those replacement
globals contain recorder objects: name lookup returns a stringifier, attribute/item/operator
access records more source shape, and results later transmogrify into strings or `ForwardRef`
objects.

That fallback assumes `annotate.__code__` is enough to execute the annotation function. A SOAC JIT
function's callable behavior is registered on the original Python function object, not on an
arbitrary `FunctionType` clone made from its visible code object. Relying on that fallback will
execute the wrong body or fail to enter the SOAC runtime.

Therefore SOAC-generated annotation helpers should implement the annotation formats they claim to
support directly.

## Implementation Plan

1. Represent annotation entries once.

   Introduce a small internal representation for annotation entries: output key, value expression,
   and original source string. Reuse it for module variable annotations, class variable annotations,
   function parameter annotations, and return annotations.

2. Keep module and class helper storage.

   Continue appending a module-level `def __annotate__(format): ...` when a module has deferred
   variable annotations.

   Continue appending `def __annotate_func__(format): ...` to class bodies. CPython's heap-type
   descriptor will expose it as `Class.__annotate__` and will call/cache it for
   `Class.__annotations__`.

3. Collect and attach function signature annotation helpers.

   Before the function body is lowered to BlockPy, collect annotations from
   `func.parameters` and `func.returns`.

   Synthesize a helper definition in the defining scope, for example:

   ```python
   def _dp_annotate_some_function(_dp_format, __soac__=...):
       ...
   ```

   Visit/rewrite/lower that helper exactly like other nested helper functions so closure captures
   are represented by the normal `make_function` path.

   When constructing the original function, pass the annotation helper expression as the fifth
   argument to `__soac__.make_function(function_id, kind, closure, defaults, annotate_fn)`.

4. Implement VALUE and STRING directly.

   VALUE format, integer 1, returns evaluated annotation values.

   STRING format, integer 4, returns the stored source strings. Do not ask annotationlib to rerun
   the function with fake globals merely to recover strings that SOAC already has.

5. Implement a conservative FORWARDREF format directly.

   FORWARDREF format, integer 3, returns a dict containing real evaluated values where evaluation
   succeeds.

   For each annotation whose evaluation fails with a name/attribute/import-time forward-reference
   style exception, return:

   ```python
   annotationlib.ForwardRef(source, module=__name__, owner=None)
   ```

   Start with whole-expression fallback. For an annotation source such as `sequence_b[int]`, if
   the expression cannot be evaluated, return `ForwardRef("sequence_b[int]", module=__name__)`.

   This is less precise than annotationlib's partial fake-globals evaluation, but covers the
   current class/function deferred annotation shapes without bytecode fallback.

6. Add a real annotation-mode lowerer if whole-expression FORWARDREF is insufficient.

   If tests require partial evaluation, add an explicit lowering mode for annotation expressions.
   In that mode name loads call a runtime helper that returns either the real name binding or a
   SOAC recorder object. Attribute/item/call/operator operations on recorder objects build recorder
   source shape. At function return, helper results become strings or `annotationlib.ForwardRef`
   instances depending on requested format.

   This should still be lowered SOAC IR. It should not depend on annotationlib creating a
   replacement Python function from `annotate.__code__`.

7. Preserve future-annotations behavior intentionally.

   Under `from __future__ import annotations`, CPython sets up `__annotations__` for module and
   class scopes and stores strings eagerly there. Prefer emitting those `__annotations__[key] =
   source` writes for module/class variable annotations instead of manufacturing lazy helpers.

   Function signature annotations can still use an attached `function.__annotate__` helper that
   returns string values, matching CPython's lazy function-annotation path in this CPython branch.

## Validation Plan

Add focused tests first for:

- transformed function exposes callable `f.__annotate__` for parameter and return annotations.
- `f.__annotations__` calls that thunk and caches a dict.
- method docstring plus annotations both survive lowering.
- class `__dict__` contains `__annotate_func__` and `Class.__annotate__` calls it.
- `annotationlib.get_annotations(owner, format=Format.STRING)` returns source strings for SOAC
  helpers without cloning bytecode.
- `annotationlib.get_annotations(owner, format=Format.FORWARDREF)` returns `ForwardRef` objects
  for missing names, missing attributes, and uninitialized nonlocal cells.

Then un-xfail, in this order when possible:

- `method_docstring`
- `generic_io_typing`
- `class_annotations_forwardref`
- `generic_namedtuple_fields`
- `tests/test_regression_annotationlib_nonlocal.py`

Run at least:

```sh
just pytest tests/test_regression_annotationlib_nonlocal.py
just pytest tests/test_integration_cases.py
cargo check -p soac-blockpy -p soac_pyo3 -p soac_jit
```

