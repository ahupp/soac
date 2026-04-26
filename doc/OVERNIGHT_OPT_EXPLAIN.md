# Overnight Optimization Explanations

This is a companion to `doc/OVERNIGHT_PERF_LOG.md`. It covers the optimizations
that were kept during the overnight pystone pass, in log order. Some were
stepping stones that are now partly or fully subsumed by later changes; those
cases are called out explicitly so this file describes the current tree, not
only the intermediate benchmark state.

The common shape is:

- `soac_opt` uses profile evidence to choose a structured optimization plan.
- `soac_ir_typed` carries that choice as typed plan or emission data.
- `soac_jit` emits the chosen operation mechanically and falls back to the
  normal Python path when a guard fails.
- `soac_jit_runtime` and `soac_py/src/soac/runtime.py` provide the low-level
  runtime helpers used by generated code.

Metric notes: throughput is the specialized apply-pass median from
`doc/OVERNIGHT_PERF_LOG.md`. Deltas are relative to that entry's recorded
baseline; when a confirmation run exists, this file uses the confirmation
result. Code size is the summarized pystone machine-code size and machine-block
count from the same benchmark entry.

# Merged

## 2. Constructor direct calls

How it works:

Profiled v3 direct-call planning can now represent constructor calls as their
own callee kind. A constructor direct call has an implicit `self` allocation
step and then calls the observed `__init__` target directly, guarded by owner
type information. The optimized path predeclares the owner type and the
`__init__` callable relocation so the process JIT can reserve those imports
before function codegen.

The fast path keeps the existing guarded constructor allocation/init machinery:
it is still a guarded specialization, not a semantic shortcut. If the type or
call target does not match the profiled shape, execution falls back to the
generic call path.

Measured effect:

- Throughput: `257595 -> 266689 loops/s` (`+9094`, `+3.53%`).
- Code size: `55082 -> 57569 bytes` (`+2487`, `+4.52%`), `3335 -> 3472`
  machine blocks (`+137`).

Where it is implemented:

- `crates/soac_ir_typed/src/plan_v3.rs`
  - `DirectCallCallee::Constructor`
  - `DirectCallSpecializationPlan`
- `crates/soac_ir_typed/src/emit_v3.rs`
  - mechanical direct-call emission data
- `crates/soac_jit/src/jit/typed_pipeline.rs`
  - `constructor_guards_for_v3_direct_call`
  - `typed_call_emission_plans_from_v3`
- `crates/soac_jit/src/jit/direct_function.rs`
  - conversion from typed direct-call plans to JIT direct-call argument plans
- `crates/soac_jit/src/jit/imports.rs`
  - predeclared owner type and `__init__` callable relocations
- `crates/soac_opt/src/call_emission_v3.rs` and `crates/soac_opt/src/typed.rs`
  - v3 direct-call plan selection and typed IR rewriting

## 3. Method direct calls

How it works:

Method calls are represented separately from ordinary function calls and
constructors. The v3 direct-call plan records a `Method` callee kind and the
constant method name. Typed planning resolves owner-type method guards, builds
an argument plan with the implicit receiver, and emits a guarded direct method
call when the receiver and selected method target match the profiled shape.

This removes a generic attribute lookup plus vectorcall for hot monomorphic
method calls such as `Record.copy`. Guard failure still falls back to the
ordinary method-call path, so dynamic method replacement and unexpected receiver
types are handled by existing Python semantics.

Measured effect:

- Throughput: `266689 -> 273261 loops/s` (`+6572`, `+2.46%`).
- Code size: `57569 -> 57953 bytes` (`+384`, `+0.67%`), `3472 -> 3498`
  machine blocks (`+26`).

Where it is implemented:

- `crates/soac_ir_typed/src/plan_v3.rs`
  - `DirectCallCallee::Method`
  - direct-call argument plan data
- `crates/soac_ir_typed/src/typed.rs`
  - `TypedDirectMethodCallGuard`
  - `TypedGuardedMethodCall`
  - `InstrTyped::GuardedMethodCallTyped`
- `crates/soac_jit/src/jit/typed_pipeline.rs`
  - `method_guards_for_v3_direct_call`
  - `typed_call_emission_plans_from_v3`
- `crates/soac_jit/src/jit/direct_function.rs`
  - direct method-call lowering for JIT planning
- `crates/soac_jit/src/jit/imports.rs`
  - owner-attribute callable relocations
- `crates/soac_opt/src/call_emission_v3.rs` and `crates/soac_opt/src/typed.rs`
  - direct-call planning and typed IR application

## 5. Cranelift `speed_and_size` default

How it works:

The default Cranelift optimization level for normal runtime and benchmark JIT
codegen is now `speed_and_size`. The JIT still accepts `SOAC_CRANELIFT_OPT_LEVEL`
to override this, but the unset default is size-aware optimization instead of
plain `speed`.

This helped pystone because the hottest generated functions had large enough
basic-block and helper-call footprints that smaller code was better for the
instruction cache without losing the important scalar optimizations.

Measured effect:

- Throughput: `275603 -> 276598 loops/s` (`+995`, `+0.36%`).
- Code size: no logged change; the next baseline records `57953 -> 57953 bytes`
  (`+0`, `+0.00%`) and `3498 -> 3498` machine blocks (`+0`).

Where it is implemented:

- `crates/soac_config/src/runtime.rs`
  - default parsing for unset `SOAC_CRANELIFT_OPT_LEVEL`
- `crates/soac_jit/src/config.rs`
  - `SoacCraneliftConfig::from_config_with_pic`
  - Cranelift ISA flag construction
- `Justfile`
  - benchmark recipe default for `CRANELIFT_OPT_LEVEL`
- `README.md` and `AGENTS.md`
  - environment-variable documentation

## 11. Return materialized `None` as immortal

How it works:

`None` is an immortal singleton in the generated-code ownership model. When JIT
code needs a materialized Python object for `None`, it now returns an
`EmitResult` marked as immortal instead of treating the value as newly owned and
emitting an `INCREF(None)`. When the result is only needed for effect, codegen
can return `NoValue` and avoid materializing `None` at all.

This removes both direct refcount calls and follow-on ownership bookkeeping at
statement-shaped sites.

Measured effect:

- Throughput: `408849 -> 411819 loops/s` (`+2970`, `+0.73%`).
- Code size: `58214 -> 58214 bytes` (`+0`, `+0.00%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `crates/soac_jit/src/jit/mod.rs`
  - `emit_none_for_demand`
- `crates/soac_jit/src/jit/typed_value.rs`
  - `EmitResult::immortal_pyobject`
  - `EmitResult::NoValue`
  - `ValueOwnership::Immortal`

## 12. Resolve Python C-API JIT symbols directly

How it works:

The JIT now tries to register direct CPython C-API symbols with Cranelift by
resolving them from the running process through `dlsym(RTLD_DEFAULT)`. If a
symbol is unavailable, SOAC keeps the existing wrapper fallback. Direct symbol
registration removes one layer of SOAC wrapper calls for common C-API helpers
while preserving compatibility with environments where a particular symbol is
not externally resolvable.

Measured effect:

- Throughput: `411819 -> 414928 loops/s` (`+3109`, `+0.75%`).
- Code size: `58214 -> 58214 bytes` (`+0`, `+0.00%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `crates/soac_jit/src/jit/specialized_helpers.rs`
  - `load_python_capi_symbol`
  - `python_capi_symbol_or_wrapper`
  - JIT symbol registration for CPython helpers

## 20. Skip global-load module-constant name decrefs

How it works:

Global load helper paths use module-constant Python string objects for the
global name. Those name objects are not owned temporaries produced by the load
site, so generated code no longer emits DECREFs for them on global-load paths.
The loaded value still follows its normal ownership rules, and store paths that
create or own name objects remain separate.

This removes refcount calls around hot module-global reads without changing the
helper's lookup semantics.

Measured effect:

- Throughput: `510936 -> 525189 loops/s` (`+14253`, `+2.79%`).
- Code size: `58613 -> 58530 bytes` (`-83`, `-0.14%`), `3554 -> 3496`
  machine blocks (`-58`).

Where it is implemented:

- `crates/soac_jit/src/jit/mod.rs`
  - global name load emission paths, including non-local and indexed global load
    helper calls
- `crates/soac_jit/src/jit/intrinsics.rs`
  - related global store/load intrinsic code; stores remain explicit about owned
    operands

# Remaining

## 1. Inline compact-int fast path for unary `not`

How it works:

`UnaryOpKind::Not` now has a local fast path when the operand is an exact compact
`PyLong`. The JIT guards the operand's type against `PyLong_Type`, checks the
compact-long tag, extracts the `i64` payload, compares it with zero, and
materializes the corresponding Python boolean. If any guard fails, execution
branches to the existing generic truthiness path.

This is useful for pystone because direct calls such as `Func2(...)` return the
module's `TRUE` and `FALSE` integer globals, and `BoolGlob = not Func2(...)`
previously paid the generic Python truthiness cost even when the value was a
small exact `int`.

Measured effect:

- Throughput: `582486 -> 587402 loops/s` (`+4916`, `+0.84%`).
- Code size: `59812 -> 60311 bytes` (`+499`, `+0.83%`), `3586 -> 3609`
  machine blocks (`+23`).

Where it is implemented:

- `crates/soac_jit/src/jit/intrinsics.rs`
  - `emit_not_with_compact_long_fast_path`
  - `emit_guarded_compact_long_i64`
  - the `UnaryOpKind::Not` branch that selects the fast path before falling back
    to generic truthiness

## 4. Slotted runtime range helpers

How it works:

The intermediate runtime `range` helper used fixed `__slots__` on the Python
runtime range object and its iterator. That removed instance dictionaries from
these helper objects and made attribute layout cheaper and more predictable.

This was later largely superseded by exporting CPython's native `range` object
from `soac.runtime`. In the current tree, `soac.runtime.range` is
`builtins.range`; the older slotted helper is no longer the primary path. The
important historical point is that range-loop performance was first improved by
making the Python helper objects simpler, then improved again by eliminating
those helper objects for normal `range` use.

Measured effect:

- Throughput: `273261 -> 275603 loops/s` (`+2342`, `+0.86%`).
- Code size: `57953 -> 57953 bytes` (`+0`, `+0.00%`), `3498 -> 3498`
  machine blocks (`+0`).

Where it is implemented:

- `soac_py/src/soac/runtime.py`
  - current binding: `range = _builtins.range`
  - current runtime still exports native `iter` and `next` as runtime helpers
- `tests/test_runtime_builtin_primitives.py`
  - coverage for the current native runtime builtin behavior

## 6. Guarded raw indices for exact-list item access

How it works:

When profile evidence and v3 planning select exact-list item access, the JIT can
emit the index as a guarded raw `i64` instead of materializing a Python integer
object and then asking CPython to interpret it. The generated path guards the
index expression as an exact compact integer, extracts the integer payload, and
uses that raw value in exact-list getitem/setitem codegen.

The specialization is still guarded. Non-int indices, non-compact integers,
unexpected list shapes, and semantic corner cases fall back to the normal Python
item access path.

Measured effect:

- Throughput: `276598 -> 278007 loops/s` (`+1409`, `+0.51%`).
- Code size: `57953 -> 57998 bytes` (`+45`, `+0.08%`), `3498 -> 3489`
  machine blocks (`-9`).

Where it is implemented:

- `crates/soac_jit/src/jit/mod.rs`
  - `typed_expr_can_emit_guarded_i64_index`
  - `emit_typed_guarded_i64_index_with_local_env`
- `crates/soac_jit/src/jit/operation_specializations.rs`
  - `lowering_plan_from_typed_exact_list_item`
  - `emit_exact_list_item_getitem_from_guarded_i64_index`
  - `emit_exact_list_item_setitem_from_guarded_i64_index`
- `crates/soac_opt/src/pipeline_v3.rs`
  - derives exact-list item specialization requests from profile evidence

## 7. Exact-string compare-to-bool branches

How it works:

The profiler records exact string operand shapes for hot comparison operators.
The v3 planner can then select exact-string comparison alternatives for branch
or bool-producing regions. The selected plan carries exact `str` guards and a
`PyObjectRichCompareBool` operation that returns an `i32` boolean value for the
branch, avoiding Python bool object traffic in the hot path.

The first kept version established the planning and typed emission shape. A
later kept change replaced the generic rich-compare call inside this operation
with direct exact-Unicode comparison via `PyUnicode_Compare`.

Measured effect:

- Throughput: `278007 -> 281575 loops/s` (`+3568`, `+1.28%`).
- Code size: `57998 -> 58214 bytes` (`+216`, `+0.37%`), `3489 -> 3497`
  machine blocks (`+8`).

Where it is implemented:

- `crates/soac_jit/src/jit/intrinsics.rs`
  - exact type-tag counter recording for `str`
- `crates/soac_opt/src/operator_specialization.rs`
  - exact operator shape tags
- `crates/soac_opt/src/evidence_v3.rs`
  - turns profile evidence into exact-string planner facts
- `crates/soac_opt/src/alternatives_v3.rs`
  - exact-string comparison alternatives
- `crates/soac_opt/src/planner_v3.rs`
  - `match_exact_str_compare_branch`
  - `match_exact_str_compare_return`
  - exact-string comparison plan construction
- `crates/soac_ir_typed/src/plan_v3.rs` and `crates/soac_ir_typed/src/emit_v3.rs`
  - `PyObjectRichCompareBool`
- `crates/soac_jit/src/jit/mod.rs`
  - mechanical emission for `PyObjectRichCompareBool`

## 8. Native iterator for runtime range

How it works:

The intermediate runtime range object was changed so `__iter__` delegated to
CPython's native range iterator. That let loops use CPython's compact
`range_iterator` state instead of a Python-level iterator object with Python
attribute updates.

This was later superseded by exporting native `builtins.range` directly from
`soac.runtime`, but it remains part of the sequence of kept range-loop
optimizations and explains why later vectorcall specialization targets
`PyRangeIter_Type`.

Measured effect:

- Throughput: `281575 -> 323300 loops/s` (`+41725`, `+14.82%`).
- Code size: `58214 -> 58214 bytes` (`+0`, `+0.00%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `soac_py/src/soac/runtime.py`
  - current `range = _builtins.range` binding
  - current `iter = _builtins.iter` binding
- `crates/soac_jit/src/jit/specialized_helpers.rs`
  - later fast path recognizes `PyRangeIter_Type`

## 9. Native `range` object for transformed builtin range

How it works:

`soac.runtime.range` now directly re-exports CPython's `builtins.range`. Code
lowered through the runtime helper therefore creates native range objects
instead of SOAC-specific Python wrapper objects. That gives transformed code the
same iteration object, storage layout, and C-level behavior that CPython uses.

This also makes downstream JIT/runtime fast paths more uniform: range loops
operate on `PyRangeIter_Type` and can use the same shape as ordinary CPython
range iteration.

Measured effect:

- Throughput: `323300 -> 402103 loops/s` (`+78803`, `+24.38%`).
- Code size: `58214 -> 58214 bytes` (`+0`, `+0.00%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `soac_py/src/soac/runtime.py`
  - `range = _builtins.range`
- `tests/test_runtime_builtin_primitives.py`
  - runtime builtin primitive coverage

## 10. Inline-values-only indexed field helpers

How it works:

Indexed field access specializes hot attribute loads and stores for objects
whose dictionaries are still in CPython's inline-values form. The helper path
checks the object dictionary layout and split-table slot state, performs the
field probe or store directly when the inline-values assumptions hold, and
falls back when the object has a materialized dict or an unexpected layout.

This kept the fast path narrow: it avoided trying to optimize materialized-dict
objects inside the helper, which would add branches to the pystone hot case.
Later trusted and inlined field helpers build on the same inline-values
assumption after stronger type/version guards.

Measured effect:

- Throughput: `402103 -> 408849 loops/s` (`+6746`, `+1.68%`).
- Code size: `58214 -> 58214 bytes` (`+0`, `+0.00%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `crates/soac_jit_runtime/src/lib.rs`
  - inline-values indexed field probe/store helpers
  - trusted inline-values store helper variants
- `crates/soac_jit/src/jit/imports.rs`
  - runtime helper import specs
- `crates/soac_jit/src/jit/symbols.rs`
  - runtime helper symbols
- `crates/soac_jit/src/jit/mod.rs`
  - typed indexed getattr/setattr emission
- `crates/soac_opt/src/pipeline_v3.rs`
  - indexed-field specialization planning

## 13. Exact compact-ASCII `ord()` helper fast path

How it works:

Static direct-call recognition maps `ord(x)` to a runtime primitive returning an
`i64`. The runtime helper first checks whether `x` is an exact compact ASCII
Unicode object of length one. If so, it reads the one-byte character payload
directly and returns the code point. Otherwise it falls back to public Unicode C
API calls for length and character extraction, preserving error behavior for
unsupported inputs.

Measured effect:

- Throughput: `414928 -> 415620 loops/s` (`+692`, `+0.17%`).
- Code size: `58214 -> 58214 bytes` (`+0`, `+0.00%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `crates/soac_jit/src/jit/direct_abi.rs`
  - `RuntimePrimitiveId::BuiltinOrdI64`
  - `runtime_primitive_for_builtin_name_and_arity`
- `crates/soac_jit/src/jit/mod.rs`
  - runtime primitive emission and result facts
- `crates/soac_jit_runtime/src/lib.rs`
  - `soac_runtime_builtin_ord_i64`
  - compact-ASCII Unicode payload read
- `doc/RUNTIME_FUNCTIONS.md` and `doc/SPECIALIZATION.md`
  - runtime helper and specialization documentation

## 14. Fast `next(range_iterator)` vectorcall

How it works:

The SOAC vectorcall hook recognizes exact calls to `builtins.next` where the
first argument is an exact CPython `range_iterator`. It reads and updates the
raw `PyRangeIter_Type` layout directly. For non-exhausted iterators, it returns
`PyLong_FromLong(current)` and advances the iterator state. For exhausted
iterators, it either sets `StopIteration` for one-argument `next(it)` or returns
an increfed default for two-argument `next(it, default)`.

The fast path only applies to the exact builtin function and exact native range
iterator type. Everything else falls through to `_PyObject_VectorcallTstate`.

Measured effect:

- Throughput: `415877 -> 422478 loops/s` (`+6601`, `+1.59%`).
- Code size: `58214 -> 58214 bytes` (`+0`, `+0.00%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `crates/soac_jit/src/jit/specialized_helpers.rs`
  - `py_vectorcall_hook`
  - `cached_builtin_next`
  - `fast_builtin_next_range_iter`
  - `RawPyRangeIterObject`

## 15. C-level `StopIteration` exception-match fast path

How it works:

The vectorcall hook also recognizes exact calls to
`soac.runtime.exception_matches(exc, StopIteration)`. For that specific shape it
calls CPython's `PyErr_GivenExceptionMatches` directly and returns a Python bool.
This avoids a Python-level runtime helper call in lowered exception matching,
especially around iterator exhaustion paths.

Only the exact runtime helper and exact `StopIteration` type get the shortcut.
Other exception matching still uses the normal vectorcall path.

Measured effect:

- Throughput: `422478 -> 471471 loops/s` (`+48993`, `+11.60%`).
- Code size: `58214 -> 58214 bytes` (`+0`, `+0.00%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `crates/soac_jit/src/jit/specialized_helpers.rs`
  - `py_vectorcall_hook`
  - `cached_runtime_exception_matches`
  - `fast_runtime_stop_iteration_match`

## 16. Trusted inline-values indexed field helpers

How it works:

Once typed planning has already emitted an exact owner type/version guard for an
indexed field access, the runtime helper does not need to revalidate every
field-key assumption. The trusted helper variants rely on the upstream guard,
then operate directly on the inline-values slot for the selected field index.

This is intentionally narrower than a generic attribute helper. The trust comes
from the typed guard and the validated optimization plan, not from arbitrary
caller discipline.

Measured effect:

- Throughput: `471471 -> 491701 loops/s` (`+20230`, `+4.29%`).
- Code size: `58214 -> 57806 bytes` (`-408`, `-0.70%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `crates/soac_jit_runtime/src/lib.rs`
  - trusted inline-values field helper variants
- `crates/soac_jit/src/jit/mod.rs`
  - guarded typed indexed getattr/setattr emission
- `crates/soac_jit/src/jit/imports.rs`
  - trusted helper import specs
- `crates/soac_jit/src/jit/symbols.rs`
  - trusted helper symbol names
- `crates/soac_opt/src/pipeline_v3.rs`
  - validates and selects indexed-field plans

## 17. Split trusted indexed-field store insert/overwrite branches

How it works:

The trusted indexed-field store helper has separate first-insert and overwrite
paths. The helper reads the old slot value once. If the slot is empty, it
increments the new value, stores it, and updates insertion-order metadata. If
the slot already has a value, it increments the new value, stores it, and
decrements the old value.

Splitting these paths removes repeated "is the old slot null?" tests from the
hot overwrite case while keeping the first-insert bookkeeping explicit.

Measured effect:

- Throughput: `491701 -> 495495 loops/s` (`+3794`, `+0.77%`).
- Code size: `57806 -> 57806 bytes` (`+0`, `+0.00%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `crates/soac_jit_runtime/src/lib.rs`
  - `soac_runtime_store_field_indexed_inline_values_trusted`
- `crates/soac_jit/src/jit/imports.rs`
  - trusted store helper import descriptor
- `crates/soac_jit/src/jit/symbols.rs`
  - `SOAC_RUNTIME_STORE_FIELD_INDEXED_INLINE_VALUES_TRUSTED_SYMBOL`

## 18. Exact Unicode branch compare via `PyUnicode_Compare`

How it works:

After the exact-string comparison planner selects a `PyObjectRichCompareBool`
operation, JIT emission now guards both operands as exact Unicode objects and
calls `PyUnicode_Compare`. The integer result is compared with zero according
to the requested rich-compare operator. This avoids allocating or testing a
Python bool result from generic rich comparison in the hot branch path.

Guard failure still falls back to the generic rich-compare branch behavior.

Measured effect:

- Throughput: `495495 -> 502048 loops/s` (`+6553`, `+1.32%`).
- Code size: `57806 -> 57798 bytes` (`-8`, `-0.01%`), `3497 -> 3497`
  machine blocks (`+0`).

Where it is implemented:

- `crates/soac_jit/src/jit/mod.rs`
  - mechanical emission for `MechanicalCodegenOperation::PyObjectRichCompareBool`
  - exact `PyUnicode_Type` guards
  - `PyUnicode_Compare` call and signed-result branch lowering
- `crates/soac_ir_typed/src/emit_v3.rs`
  - mechanical operation definition
- `crates/soac_opt/src/alternatives_v3.rs`
  - exact-string compare alternatives that select this operation

## 19. Inline trusted indexed-field load probe

How it works:

The trusted indexed getattr path now emits the inline-values load probe directly
in generated code after the exact type/version guard. The code checks the split
dict/inline-values layout, reads the selected slot, and branches to fallback on
miss. On hit, it emits the needed incref for the loaded value and continues.

This removes a runtime helper call from hot attribute loads while preserving the
same miss/fallback behavior as the helper-based path.

Measured effect:

- Throughput: `502048 -> 510936 loops/s` (`+8888`, `+1.77%`).
- Code size: `57798 -> 58613 bytes` (`+815`, `+1.41%`), `3497 -> 3554`
  machine blocks (`+57`).

Where it is implemented:

- `crates/soac_jit/src/jit/mod.rs`
  - `emit_typed_indexed_getattr`
  - inline trusted field probe codegen
- `crates/soac_jit_runtime/src/lib.rs`
  - original helper layout and trusted store support used as the runtime ABI
    reference

## 23. Allow exact-string compare plans to read module constants

How it works:

Exact-string compare regions can now use module constants as borrowed PyObject
inputs. This lets a branch such as a local string compared with a constant
string literal enter the same exact-string compare plan as local-vs-local
comparisons. The module constant is validated by the plan machinery and emitted
as a borrowed input to the mechanical operation.

Measured effect:

- Throughput: `528248 -> 533953 loops/s` (`+5705`, `+1.08%`).
- Code size: `58424 -> 58752 bytes` (`+328`, `+0.56%`), `3486 -> 3503`
  machine blocks (`+17`).

Where it is implemented:

- `crates/soac_ir_typed/src/plan_v3.rs`
  - `RegionInputSource::ModuleConstant`
- `crates/soac_ir_typed/src/emit_v3.rs`
  - `MechanicalRegionInputSource::ModuleConstant`
  - mechanical region input validation
- `crates/soac_opt/src/planner_v3.rs`
  - region input extraction for `NameLocation::Constant`
  - exact-string compare branch/return planning
- `crates/soac_jit/src/jit/mod.rs`
  - mechanical region input emission for module constants

## 24. Let exact-string compare regions borrow indexed globals

How it works:

Exact-string compare regions can also use selected indexed global loads as
borrowed PyObject inputs. If profile/planning has selected an indexed-global
access for a module global, the exact-string compare region can consume that
borrowed value directly after the indexed-global guard rather than forcing the
region to be local-only.

On fallback, the generated code reloads the global through the normal owned
path, so the borrowed optimized input does not weaken Python fallback behavior.

Measured effect:

- Throughput: `533953 -> 541078 loops/s` (`+7125`, `+1.33%`).
- Code size: `58752 -> 59510 bytes` (`+758`, `+1.29%`), `3503 -> 3553`
  machine blocks (`+50`).

Where it is implemented:

- `crates/soac_ir_typed/src/plan_v3.rs`
  - `RegionInputSource::IndexedGlobal`
- `crates/soac_ir_typed/src/emit_v3.rs`
  - `MechanicalRegionInputSource::IndexedGlobal`
- `crates/soac_opt/src/planner_v3.rs`
  - `PyObjectRegionInputSource::IndexedGlobal`
  - `ExtractedRegionExt::pyobject_input_source`
  - fallback input handling
- `crates/soac_jit/src/jit/mod.rs`
  - `emit_borrowed_planned_indexed_global_load`
  - mechanical region input emission

## 25. Let exact-int operator regions borrow indexed globals

How it works:

The indexed-global region input support is reused for exact-int operator
regions. Hot compact-int branches and returns can now consume borrowed selected
indexed-global values, guard them as exact compact integers, unbox them, and
perform machine integer comparison or arithmetic. Without this, the same
optimization was limited to locals and missed module globals such as pystone's
integer globals.

Fallback reloads globals through the normal owned path, matching the
exact-string indexed-global behavior.

Measured effect:

- Throughput: `541078 -> 559372 loops/s` (`+18294`, `+3.38%`).
- Code size: `59510 -> 63314 bytes` (`+3804`, `+6.39%`), `3553 -> 3795`
  machine blocks (`+242`).

Where it is implemented:

- `crates/soac_opt/src/planner_v3.rs`
  - compact-int branch/return planning with indexed-global inputs
  - `PyObjectRegionInputSource::IndexedGlobal`
- `crates/soac_opt/src/evidence_v3.rs`
  - exact-int planner facts from profile evidence
- `crates/soac_ir_typed/src/plan_v3.rs` and `crates/soac_ir_typed/src/emit_v3.rs`
  - indexed-global region input representation
- `crates/soac_jit/src/jit/mod.rs`
  - borrowed indexed-global input emission and compact-int mechanical operation
    lowering

## 26. Lower sync for loops through next default sentinel

How it works:

Synchronous `for` loops now lower to a sentinel-based `next` loop instead of a
try/except around `StopIteration`. The lowering creates a fresh sentinel object,
calls `__soac__.next(iterator, sentinel)`, checks `tmp is sentinel`, and breaks
when the sentinel is returned. Async `for` remains on the exception-based
`anext`/`StopAsyncIteration` lowering.

This makes ordinary sync loops easier for the JIT to optimize because iterator
exhaustion becomes a value comparison rather than exception control flow. It
also composes with the `next(range_iterator)` vectorcall fast path, which
handles the two-argument default form directly.

Measured effect:

- Throughput: `559372 -> 574244 loops/s` (`+14872`, `+2.66%`).
- Code size: `63314 -> 59770 bytes` (`-3544`, `-5.60%`), `3795 -> 3565`
  machine blocks (`-230`).

Where it is implemented:

- `crates/soac_lowering/src/passes/ruff_to_blockpy/stmt_sequences.rs`
  - `expand_for_stmt`
- `crates/soac_lowering/src/passes/test.rs`
  - lowering coverage for sync and async loop expansion
- `crates/soac_jit/src/jit/specialized_helpers.rs`
  - two-argument default handling in `fast_builtin_next_range_iter`

## 27. Static `iter(x)` runtime primitive

How it works:

Static builtin recognition maps one-argument `iter(x)` to a runtime primitive.
The JIT emits a direct call to `soac_runtime_builtin_iter_object`, which calls
`PyObject_GetIter`, instead of emitting a generic Python vectorcall to the
runtime builtin. The direct ABI descriptor records the argument and result
shape so typed JIT planning can treat it like other statically selected runtime
primitives.

This is deliberately a primitive rather than a broad inline implementation:
`PyObject_GetIter` remains the semantic authority for arbitrary Python objects.

Measured effect:

- Throughput: `574244 -> 582486 loops/s` (`+8242`, `+1.44%`).
- Code size: `59770 -> 59812 bytes` (`+42`, `+0.07%`), `3565 -> 3586`
  machine blocks (`+21`).

Where it is implemented:

- `crates/soac_jit/src/jit/direct_abi.rs`
  - `RuntimePrimitiveId::BuiltinIterObject`
  - `runtime_primitive_for_builtin_name_and_arity`
  - runtime primitive descriptor
- `crates/soac_jit/src/jit/mod.rs`
  - static runtime primitive call selection and emission
- `crates/soac_jit_runtime/src/lib.rs`
  - `soac_runtime_builtin_iter_object`
- `soac_py/src/soac/runtime.py`
  - `iter = _builtins.iter`
- `doc/RUNTIME_FUNCTIONS.md` and `doc/SPECIALIZATION.md`
  - runtime helper and specialization documentation
