---
title: "Runtime Function Inventory"
---

# Runtime Function Inventory

This document tracks the callable runtime helper surface used by SOAC. Keep it
in sync when helpers are added, removed, renamed, or moved between the raw
runtime crate, the JIT specialized-helper layer, and the Python `soac.runtime`
module.

Scope:

- `soac_jit_runtime`: exported `#[unsafe(no_mangle)]` functions in
  `crates/soac_jit_runtime/src/lib.rs`.
- `specialized_helpers.rs`: exported helpers and symbols registered by
  `register_specialized_jit_symbols`.
- `soac_jit`: exported C ABI entrypoints registered directly by the JIT
  backend.
- `soac.runtime`: top-level Python callables, runtime classes, methods, and
  intentionally re-exported helper callables in `soac_py/src/soac/runtime.py`.
  Some of these names, such as import helpers, are native `_soac_ext`
  callables re-exported by `soac.runtime`.
- Synthetic inter-pass markers: helper-shaped names emitted by one compiler pass
  and recognized by a later pass. These names are not executable runtime APIs and
  must not survive to codegen or Python execution.

This list does not include plain runtime constants such as `TRUE`, `FALSE`,
`NONE`, `EMPTY_TUPLE`, or type/data symbols such as `PyFunction_Type`.

`dp_jit_profile_callable_function_id` observes the actual authenticated strict
function behind a function or bound method. It returns no strict identity for
ordinary, copied, replaced or terminal functions and never grants an unchecked
entry token. Protocol target probes use the same identity after callback-free
inspection of exact class-dictionary string keys. Optional probes preserve an
existing Python exception and do not perform argument binding or type checks.

`_soac_ext.strict_module_diagnostics(module)` is a read-only observation API,
not an admission or optimization capability. It authenticates the actual
native module definition, live namespace owner, interpreter, and verified
source before reporting the seal state, source digest/path, and artifact and
startup identities. Ordinary lookalike modules return `None`; failed or
terminal owned modules do not become ordinary. The strict pyperformance worker
uses this evidence before measured values, rather than inferring sealing from
an import having returned or a cache file existing.

`_soac_ext.strict_function_entry_kind(function)` authenticates a strict
function's actual native owner and reports its current public entry as
`entry_interpreter`, `checked_native`, `generator_factory`, or `public_override`.
The ordinary CPython backend instead reports `original_code`, or
`ordinary_replacement` for a permitted replacement with no original-source
execution authority.
Ordinary functions return `None`. This read-only diagnostic does not authorize
a call or bypass a check; compatibility tests use it to distinguish an actual
entry interpreter from a requested mode that eager compilation might replace.
The historical `checked_native` label identifies the authenticated native
entry; it does not mean parameters or results receive runtime type checks.

`_soac_ext.strict_function_diagnostics(function)` observes the actual ordinary-
interpreter function owner, authenticated source/generation/startup identities,
finalization state, and the native original-code entry
witness. It returns `None` for ordinary functions and for the compiled backend;
lookalike attributes cannot supply that witness. The CPython-only acceptance
tests use it with `_soac_ext.runtime_compilation_activity()`, whose three
process counters measure real lowering, BlockPy-cache and JIT-engine entries.
The immutable CPython backend rejects those compiler paths before entry; the
tests require all three counters to remain zero before and after execution.
Function diagnostics use schema 2, without a required-boundary bit; module
diagnostics retain schema 1.

`_soac_ext.strict_function_call_statistics(function)` reports an independent
dictionary snapshot of the actual authenticated function's `direct_body_calls`
and `fixed_body_calls`. There are no performed/discharged argument-check
counters. Ordinary functions return `None`. Mutating the snapshot cannot change
the counters or authorize optimization.

## Native iterator and comprehension primitives

`jit/native_iterator_runtime.rs` supplies raw primitives to the validated
version-one typed native-iterator CFG; the loop itself is emitted in JIT code.
`jit/collection_runtime.rs` shares exact consuming collection insertion with
that materializer and the explicit `ComprehensionInsert` operation. These are
not Python-callable helper bodies or a source-activation bypass. Object inputs
are borrowed unless the table explicitly says they are consumed.
The native iterator primitive inventory supplies both executable symbol
addresses and serial import declarations before parallel codegen snapshots are
frozen. Collection, call-argument and loop-step imports use the same shared
inventory rule during that phase;
workers never add missing imports to a reserved snapshot.

| Registered helper | Ownership and result |
| --- | --- |
| `dp_jit_native_iterator_guard` | Checks evaluated stage/sink objects against canonical native builtins; no callback or exception. |
| `dp_jit_native_iterator_next_slot` | Returns the borrowed iterator's current native `tp_iternext` address. |
| `dp_jit_native_iterator_filter_truth` | Borrows callback/item, owns and retires the predicate result; returns truth or `-1`. |
| `dp_jit_native_iterator_exhausted` | Accepts no error or clears `StopIteration`; leaves other errors untouched. |
| `dp_jit_native_iterator_materializer_init` | Initializes caller-owned list/tuple stack state; failure leaves it abortable. |
| `dp_jit_native_iterator_materializer_append` | Consumes an item on either edge; state owns any retained partial result. |
| `dp_jit_native_iterator_materializer_finish` | Consumes state and returns an owned list/tuple or NULL; failed tuple completion also consumes its array references. |
| `dp_jit_native_iterator_materializer_abort` | Takes and releases remaining partial results while preserving the pending exception; consumed state is inert. |
| `dp_jit_comprehension_insert` | Borrows an exact list/set/dict, consumes key/value on success or failure, and returns `0` or `-1`. Rejects wrong container types independently of the Operand storage proof. |
| `dp_jit_build_collection` | Consumes and NULLs an ordered owned-input array on either return; creates an exact list/set/dict with native build and failure-cleanup semantics. Dict inputs are interleaved key/value pairs. |
| `dp_jit_iterator_step` | Borrows a validated loop iterator, reloads its current native next slot, and returns an owned item or NULL with pending StopIteration/other error. Never clears exhaustion or consumes the iterator. |
| `dp_jit_call_argument_update` | Borrows callable and exact list/dict buffer; consumes a star/keyword update after native expansion/merge and error formatting, retaining any buffer prefix. |
| `dp_jit_call_argument_finish_list` | Consumes the taken list primary and its contents; returns an owned tuple or NULL. The caller publishes NULL before entry and a tuple only on success. |
| `dp_jit_call_argument_normalize_singleton` | Borrows callable and raw starred argument. Returns an owned exact tuple or NULL; failure leaves the raw primary owned, success replaces it before release. |
| `dp_jit_call_argument_check_prepared` | Checks the already-prepared exact tuple and optional exact dict without expansion, mutation, or ownership transfer. |
| `dp_jit_call_owned_operands` | Calls through the existing contextual vectorcall with borrowed evaluated inputs, then consumes and NULLs every owned callable/positional-input slot on either return while preserving the exception. Returns an owned result or NULL; no native stack-reference schedule is reproduced. |

The iterator emitter binds canonical `PyObject_GetIter` and map's one-argument
`PyObject_Vectorcall` without the offset flag. Filter uses native
`PyObject_CallOneArg` semantics. Native list append, set add, and dictionary
insertion steal their item inputs; the list fast path shares the native
resize export. No operation looks up `.append`, `.add`, or an internal helper
through mutable Python mappings. Container borrowing cannot overlap a nested
`TakeOperand` of that same physical owner; the shared IR validator rejects it
before evaluating any child. `TakeOperand` itself emits a checked raw owner
move and clear, without a new runtime helper or reference-count action.
Local and preserved Operand locations use the same explicit role validation.
The preserved-state runtime's checked owner-slot accessor additionally checks
the actual live capsule's fixed role table before an interpreter-path transfer.
Prepared calls use `PySoac_ObjectCallWithContext` with these existing inputs,
without a second star expansion or keyword merge.

These paths are under integration. Structured/native-kernel gates do not
replace matching transformed-runtime validation. New optimization and
performance work remain deferred by `OPT_GOAL.md`.

## soac_jit_runtime

Exported C ABI helpers:

```text
soac_runtime_decref_dealloc_preserving_error
soac_runtime_decref
soac_runtime_incref
soac_runtime_decref_applied
soac_runtime_incref_applied
soac_runtime_set_raised_exception
soac_runtime_tuple_new
soac_runtime_tuple_set_item_stolen
soac_runtime_example_known_value_source
soac_runtime_example_offset_known_value
soac_runtime_builtin_ord_i64
soac_runtime_builtin_len_i64
soac_runtime_builtin_iter_object
soac_runtime_unpack_fixed
soac_runtime_builtin_chr_i64
soac_runtime_pylong_as_i64
soac_runtime_pylong_as_i64_saturating
soac_runtime_count_affine_distinct_permutations_i64
soac_runtime_probe_global_indexed
soac_runtime_load_global
soac_runtime_store_global_indexed
soac_runtime_store_global_indexed_stolen
soac_runtime_store_global
soac_runtime_probe_field_indexed
soac_runtime_store_field_indexed
soac_runtime_probe_field_indexed_inline_values
soac_runtime_store_field_indexed_inline_values
soac_runtime_probe_stable_indexed_field
soac_runtime_load_native_object_slot
soac_runtime_compare_compact_ascii_unicode
```

The two `soac_runtime_store_field_indexed*` helpers require the actual inline
values state to be live/writable (1). Native ordinary-dictionary preparation
uses the transient state 2: read probes still observe its live values, but raw
writers decline and leave the receiver unchanged. This safety guard adds no
layout, checked-field, or specialization authority; the checked native path
still owns any write decision.

`soac_runtime_unpack_fixed(tstate, iterable, arity)` implements the resolved,
compiler-owned fixed-length assignment-unpack operation. Its direct ABI
borrows the iterable, accepts an unboxed integer target count, and returns an
owned tuple or null with the current Python exception set. An exact tuple of
the required length is returned with a new reference; an exact list of the
required length is snapshotted with `PyList_AsTuple`. Wrong-length values,
tuple/list subclasses, and other iterables use the registered
`dp_jit_unpack_fixed_slow` helper below. Ordinary user calls and starred
assignment unpacking retain the existing Python-visible `unpack` behavior.
The compiler-owned `UnpackFixed` identity survives typed expression
linearization; nested argument evaluation must not replace it with a captured
Python callable. Replacing `soac.runtime.unpack` therefore affects explicit
helper calls, not fixed-length assignment operations.

`soac_runtime_load_global(globals, builtins, name, expected_index)` receives
the current function's captured builtins mapping explicitly, rather than
re-reading mutable `globals["__builtins__"]`. An indexed globals hit returns
an owned value immediately; misses delegate to the corresponding
`soac_runtime_load_global_slow(globals, builtins, name, expected_index)`
helper below. Indexed dictionaries keep an immutable name/index prefix
descriptor separate from their visible lookup keys. Their values header holds
capacity, visible-order size, and the prefix pointer; the authoritative values
start at byte 24 on the supported 64-bit ABI. Unicode and general overflow
tables retain the same prefix indices through growth, deletion, and reinsertion.
Uninitialized/deleted prefix slots are NULL and invisible to ordinary lookup.
Native type owners can use `_PyDict_NewFromIndexedSchema(template)` to share
only an immutable prefix through a normal GC-owned template edge; the fresh
dictionary inherits no values, overflow entries, or policy authority.
The raw probe checks the expected name and reserved index and requires the
positive `NoLookupAliases` guard: a stored custom key may change equality
without mutating the dictionary, so an alias-sensitive dictionary always uses
full lookup. The slow helper repeats that guard rather than turning a raw
probe miss back into an unchecked physical load. Validated indexed misses check captured builtins directly;
exact-dictionary fallback uses `PyDict_GetItemRef`, while subclasses/custom
mappings use `PyMapping_GetOptionalItem`. All paths preserve globals-first
precedence, mapping callbacks, exceptions, and missing-name behavior;
specialized NULL results propagate their existing `NameError`.
The prefix/alias and exact-name checks use self-contained raw-runtime macros:
the CLIF-producing backend can outline Rust functions despite `inline(always)`,
and this ABI bundle intentionally links only exported runtime symbols and C
externs. A structured test validates the callable-symbol closure of the actual
emitted CLIF so a missing private helper is detected before module execution.

Both indexed-global store helpers use the authoritative native setter, including
the stolen-value variant. Profile/index facts never authorize a raw slot write:
native policy checks, first-insert ordering, watchers, and finalizers still run.
A native policy or callback error is returned without a fallback retry.
Split-field store probes decline dictionaries carrying a native policy, leaving
descriptor precedence and checked writes to the ordinary attribute path.

`soac_runtime_probe_stable_indexed_field(receiver, expected_type, name, index,
default_mro_index, default_namespace_index)` is a separate stable-prefix load
kernel, not the legacy split/inline-values probe. A successful private sealed
capability match must dominate it with no intervening Python effect. It checks
the exact receiver type, current indexed representation, reserved name/index,
and positive `NoLookupAliases`, then reloads the current authoritative values
base. A populated slot returns a borrowed value; the caller must INCREF it
before releasing inputs or allowing effects. NULL/UNSET, alias-sensitive
dictionaries, ordinary subclasses, and failed guards use the original getattr
operation before any user callback has run. Both default indices are `-1` for
no class binding; otherwise they identify an actual frozen heap-class MRO entry
and its native namespace prefix index, not its dictionary iteration position.
The current default object's descriptor slots are rechecked even when the
instance slot is populated. The kernel never returns a class default directly
and confers no checked-value or scalar-representation proof. Native annotation
cache bindings and static builtin default namespaces are excluded at publication.

`soac_runtime_load_native_object_slot(receiver, expected_type, offset)` is the
raw load for a construction-bound `NativeObjectMember` field capability. The
offset comes from the actual native `T_OBJECT_EX` member catalog, independently
of logical field and dictionary-prefix indices. A dominating exact-construction
match authorizes it; codegen selects this operation from the immutable
capability's storage variant, never from a source annotation or profile offset.
It returns a borrowed value or NULL for the original attribute path, so deletion
and unbound slots retain normal `AttributeError` behavior. It has no dictionary
lookup, Python callback, allocation, or reference-count operation; the caller
INCREFs a hit before any effect. Ordinary subclasses retain generic reads even
though their physical native writes inherit the declaring field policy.

## specialized_helpers.rs

Direct exported helpers:

```text
soac_runtime_set_runtime_error_static
soac_runtime_load_global_slow
dp_jit_enter_recursive_call
dp_jit_handled_state_init
dp_jit_handled_state_select
dp_jit_handled_state_raised
dp_jit_handled_state_finish
dp_jit_handled_state_release_residual
dp_jit_retire_terminal_roots
dp_jit_reraise_current
dp_jit_restore_raised_exception
dp_jit_generator_return
dp_jit_record_top_value_sample
dp_jit_profile_callable_function_id
dp_jit_protocol_iter_function_id
dp_jit_protocol_next_function_id
dp_jit_load_runtime_obj
dp_jit_pyobject_getattr
dp_jit_pyobject_setattr
dp_jit_pyobject_getitem
dp_jit_pyobject_setitem
dp_jit_pyobject_delitem
dp_jit_pytype_generic_alloc
dp_jit_finish_constructor_init
dp_jit_store_global
dp_jit_del_global
dp_jit_del_global_quietly
dp_jit_del_quietly
dp_jit_pyobject_to_i64
dp_jit_make_cell
dp_jit_raise_unbound_local_error
dp_jit_raise_missing_required_argument
dp_jit_preserved_values_ptr
dp_jit_del_preserved
dp_jit_del_preserved_quietly
dp_jit_load_cell
dp_jit_store_cell
dp_jit_del_deref
dp_jit_del_deref_quietly
dp_jit_deopt_resume
dp_jit_dict_new
dp_jit_dict_set_item
dp_jit_is_true
dp_jit_raise_i64_overflow
```

The handled-state helpers consume explicit ordered handler regions from the
resolved BlockPy control flow. `init` uses the current native exception item for
normal calls; suspended bodies instead own a stable, GC-traversed item in their
preserved state and link it only during a resume. Their exact original region
layout comes from the current function's immutable deopt table, with original
IDs preceding any native-only inlined regions. Resumes compare that ordered
layout before reusing saved state. `raised` distinguishes a new entry into the
same lexical handler from a continuation whose current exception may have been
changed through the C API. `select` first removes exited regions before source
local cleanup, then enters new regions at their target block. It saves the
actual current item value, not the resolved topmost inherited exception.
Its final ABI operand explicitly selects Leave, Enter, or Unwind. Unwind trims
only existing records, without entering a fresh handler or consuming the
pending raised-scope marker; pending finally owners can therefore be released
between successive enclosing-state restorations.
`dp_jit_handled_state_finish(state, yielded, preserved)` receives the actual
preserved capsule, or NULL for an ordinary function. A terminal managed step
notifies native closed state before any handled-item release can call Python.
The binding owns a rejected-notification error through cleanup; the outer
resume consumes it and any otherwise-successful result exactly once. `finish`
retires semantic handler scopes while the activation's item is still linked,
then detaches it and restores the caller before frame-local cleanup. It does
not release a suspended item's residual C-API-installed exception yet.
`dp_jit_handled_state_release_residual(state)` releases that residual after
ordinary local cleanup. Yields detach without retiring their suspended scopes
or residual. No frame-retention or implicit-finalizer timing parity is promised.
Deoptimization borrows this original activation and returns its final result or
error without replaying a compiled exception edge. `reraise_current` implements
bare `raise` from the current handled exception rather than a caught-name alias.
These paths preserve the independent raised-error indicator across decrefs.
Generator/coroutine terminal blocks explicitly finish the suspended activation
before deleting saved locals. `TermRaise::disposition` selects the existing
`restore_raised_exception` operation for normalized forwarding, independently
of whether that block detaches a suspended activation. The same operation
forwards normalized finally and ordinary escaping errors unchanged;
unlike a new source raise, it does not attach the resuming caller as a new
implicit `__context__`. PEP 479 cause/context construction remains separate.
The distinct `GeneratorReturn` terminator keeps its evaluated return value
owned until all frame roots have been released. `dp_jit_generator_return`
consumes that owned value and returns NULL with the completion exception set:
None uses CPython's `PyErr_SetNone(StopIteration)` caller-context semantics;
other values construct StopIteration with one argument and install it directly,
preserving tuple/exception value identity without implicit re-chaining. Any
allocation error is preserved while the return value is released. The entry
and deopt interpreters carry the same explicit completion outcome through
frame cleanup before invoking this helper; a source-raised StopIteration is
still an ordinary raise subject to PEP 479.

The 2026-08-25 (PDT) amendments remove SOAC traceback/frame reconstruction and
observer-only compatibility/refusal machinery. There is no native observer
scope or reservation API. Ordinary CPython observers remain unchanged; SOAC
event coverage may be absent or incomplete. Source authentication, metadata
mutation barriers, native recursion checks and internal SOAC counters remain
independently required.

Error codegen queues cold failure blocks until the originating block's
terminator is complete. Each queue entry captures typed, borrowed SSA inputs
and their local ownership state, not newly acquired handler arguments.
Exception forwarding, including its INCREFs and scalar boxing, runs only after
the error edge is taken. The handled-region entry selector uses the same path.
Fallible materialization uses the ordinary local cleanup continuation;
successful source paths do not acquire exception-only owners.
The validated dispatch sidecar distinguishes owning target arguments from
borrowed aliases. Borrowed-only forwarding requires an actual borrowed boxed
source; mixed consumers clone only the additional owning targets. Both
forwarding loops track newly acquired references separately from original
owners, so a later boxing failure releases the prepared prefix exactly once
through the existing exception-preserving decref path. This is a codegen
ownership distinction, not a new runtime helper or a generator-name rule.
Deopt completion results own their values until ordinary call or generator
completion consumes them. Discarding a result preserves the pending exception.
`dp_jit_retire_terminal_roots(environment)` releases completed invocation,
suspended-snapshot and binding function references. It unpublishes nullable owner
slots before callbacks, and preserves the raised error. Repeated terminal calls
are harmless; yields retain suspension ownership. This is ordinary cleanup,
not a native frame handoff or a finalizer-schedule proof.

`dp_jit_deopt_resume` consumes each owned entry in its validated live-value
buffer exactly once. After validating the table, ordinal, and buffer shape,
it records one cold `deopt_entry_guard_miss` event in that actual compiled
table's module-owned atomic counter set. The event precedes interpreter
admission and needs no Python callback or allocation. Counter sets contain no
Python owners and do not resize published hot scalar-counter storage; the
existing terminal log/dump path snapshots them. Admission protects the entire buffer before inspecting
individual entries, including the unvisited tail after a NULL scalar-boxing
result. Locals acquire the owners only after all entries validate; a later
frame-admission failure drops them, and ordinary frame cleanup makes that drop
idempotent. Failed admission preserves an existing Python error such as
MemoryError. Its ninth ABI operand is the original strict activation, separate
from the handled-state operand; deoptimization borrows that existing call
for source authentication, without reconstructing frames
or recovering compiler context through ambient state. Entry interpretation has a different input contract: its caller
retains and releases the complete buffer if local admission fails.

`dp_jit_load_cell(cell, name, binding_kind)` reads a Python cell and returns an
owned value. `name` is the original exact Unicode binding name; kind 0 is an
owned local binding and kind 1 is a captured free binding. Name binding records
both on `Load.cell_binding`, and inline/storage remaps preserve them. Empty
cells raise CPython's corresponding `UnboundLocalError` or `NameError`, including
the free-variable exception's `.name`; an existing exception is not replaced.
The same helper serves compiled loads, exact-int `CellValue` inputs, and the
entry interpreter. Missing or invalid binding metadata fails explicitly rather
than guessing from the remapped physical cell location.

Membership `BinOpKind::Contains` keeps source operands in needle/container
order through evaluation. Only the `PySequence_Contains` ABI boundary swaps
those values to container/needle; cleanup releases the container before the
needle, including when the containment protocol raises. The entry interpreter
uses the same source-order IR convention.

Registered Rust-owned cold helper:

```text
dp_jit_unpack_fixed_slow
```

`dp_jit_unpack_fixed_slow(tstate, iterable, arity)` delegates generic
fixed-length extraction to CPython's `_PyEval_UnpackIterableStackRef`. Its
independent Rust-owned stack-reference buffer preserves iterator callbacks,
exact CPython arity diagnostics, and cleanup without exposing a partially
initialized, GC-tracked Python tuple to arbitrary iterator code. CPython
writes stack references in reverse order; the helper restores item order and
uses `_PyTuple_FromStackRefStealOnSuccess` to publish the owned result. Failed
tuple construction releases still-owned tagged stack references exactly once.
The same cold operation serves native guard misses and the entry/deopt
interpreter; the registered symbol is not a Python runtime API.

Perf-frame toggle helper pairs:

```text
dp_jit_raise_from_exc
dp_jit_raise_from_exc_with_frame
dp_jit_guard_method_type_version
dp_jit_guard_method_type_version_with_frame
dp_jit_py_call_positional_three
dp_jit_py_call_positional_three_with_frame
dp_jit_py_call_object
dp_jit_py_call_object_with_frame
dp_jit_get_arg_item
dp_jit_get_arg_item_with_frame
```

Generic borrowed vectorcall and tuple/keyword calls use the explicit
`PySoac_VectorcallWithContext` and `PySoac_ObjectCallWithContext` entrypoints
below. The narrow owned-Operand positional path uses the same context-aware
vectorcall, then releases its owned inputs on either success or failure. These calls
do not pass through a separate SOAC builtin classifier or ad hoc
`next`, `any`, `all`, or exception-matching shortcut. The remaining call pairs
serve runtime-operation sites with the same explicit native context:
`dp_jit_py_call_positional_three(callable, arg1, arg2, arg3, globals, namespace, builtins)`
accepts up to three positional arguments (trailing NULL means absent), while
`dp_jit_py_call_object(callable, tuple, globals, namespace, builtins)` passes no keywords.
Both delegate to the native context-aware public call path. An actual builtin
resolved by a runtime-helper lookup therefore cannot use an unrelated Python
frame, and a strict function still enters its authenticated public trampoline.
Globals and captured builtins come from the same existing function environment;
rebinding the module's `__builtins__` does not replace a function's captured mapping.

Registered CPython-wrapper call targets:

```text
PyObject_RichCompare
PyUnicode_Compare
PySequence_Contains
PyLong_FromLongLong
PyObject_Not
PyObject_IsTrue
PyNumber_Add
PyNumber_Subtract
PyNumber_Multiply
PyNumber_MatrixMultiply
PyNumber_TrueDivide
PyNumber_FloorDivide
PyNumber_Remainder
PyNumber_Power
PyNumber_Lshift
PyNumber_Rshift
PyNumber_Or
PyNumber_Xor
PyNumber_And
PyNumber_InPlaceAdd
PyNumber_InPlaceSubtract
PyNumber_InPlaceMultiply
PyNumber_InPlaceMatrixMultiply
PyNumber_InPlaceTrueDivide
PyNumber_InPlaceFloorDivide
PyNumber_InPlaceRemainder
PyNumber_InPlacePower
PyNumber_InPlaceLshift
PyNumber_InPlaceRshift
PyNumber_InPlaceOr
PyNumber_InPlaceXor
PyNumber_InPlaceAnd
PyNumber_Positive
PyNumber_Negative
PyNumber_Invert
```

Registered call targets implemented outside `specialized_helpers.rs`:

```text
dp_jit_checked_function_metadata
dp_jit_vectorcall_bind_direct_args
dp_jit_vectorcall_compile_function_env
dp_jit_strict_finish_call
dp_jit_prepare_strict_direct_call
dp_jit_finish_strict_direct_call
dp_jit_retire_strict_call_arguments
dp_jit_vectorcall_previous_for_changed_code
dp_jit_direct_compile_function_env
soac_jit_make_function_with_closure
soac_jit_complete_function_definition
soac_jit_construct_class
soac_jit_resume_generator
```

`dp_jit_checked_function_metadata(function)` verifies the opaque metadata's
owning destructor before generated code reads any private payload field. A
successful lookup is callback-free; a missing or foreign payload returns NULL
with an error, whose construction may allocate. Destructor identity proves the
payload's allocation type, not source authority. Observational queries treat
foreign metadata as a miss and preserve an existing exception.
Native `PyFunction_SetSoacMetadata` publishes the complete pointer/destructor/ID
association before retiring the old payload. A destructor may reenter the
setter; the nested replacement or clear remains installed, and no displaced
payload is silently overwritten. This opaque storage operation does not
change the function's source owner or permanent metadata seal.

`dp_jit_vectorcall_bind_direct_args` completes normal Python argument binding.
It returns the environment to use for this call
and writes an optional owned activation token. Strict calls look up only missing
defaults, in Python parameter order, from the function's then-current metadata.
Keyword-default lookups propagate native dictionary errors and reload the
mapping after reentrant changes. Source calls retain the original native
parameter-name objects for both explicit keywords and default lookups. Keyword
matching completes CPython's pointer-identity pass before rich equality, and
propagates equality errors before later binding errors. Generated compiler
helpers use their explicit logical signature instead of the placeholder code's
parameter list. Selected defaults become owned argument slots;
actual closure cells are pinned after binding, before body entry. Retained
compiled and entry/deopt execution use this binder; native CPython execution
uses its ordinary frame binder. The per-call environment also
retains globals and builtins; idle strict metadata retains no old defaults or
cells. `dp_jit_strict_finish_call(activation, result)` releases the captured
activation on success or failure and preserves an existing body exception;
it does not check the result's type. Replacing the code of an unadopted function
leaves its captured active invocation intact; later calls use the
replacement's ordinary entry through a retained SOAC trampoline. That trampoline
checks the live native owner, exact compiler and module-execution identities,
and the absence of a finalized metadata contract on every call; it never
publishes an unchecked native vectorcall or grants replacement code any source
facts. Unchecked direct/inline plans remain excluded for strict source modules.

`dp_jit_prepare_strict_direct_call(callable, args, nargs, out_capacity,
expected_entry, expected_body, out_args, out)` is a private source-body consumer
for an already captured function or sealed method. Its raw ABI carries the supplied
argument count separately from the full output capacity. It rechecks the actual
private/public entry and fixed-positional body ABI, then uses the ordinary
current-default binder and actual per-call environment. An unbound call passes
no earlier method-entry witness; this does not waive current-entry
authentication. A return of `0` is a miss before binding, `-1` preserves the
binding/preparation error without retry, and `1` transfers owned argument slots
and an activation into the outputs. The eight-argument ABI has no argument-proof
pointer, and no parameter or return predicates are run.
`dp_jit_retire_strict_call_arguments(activation)` clears the environment's
borrowed active-activation pointer before the binder
releases its argument references. The public trampoline and
`dp_jit_finish_strict_direct_call(activation, arguments, count, result)` preserve
this ownership/cleanup ordering.
Post-prepare errors/panics are protected by an owned preparation guard.
An optional source-selected fixed-body address is observation/guard input,
not authority to bypass binding or source ownership. The emitted direct call requires
the prepared body's address to match; otherwise the same prepared activation
is invoked indirectly. Read-only `strict_function_call_statistics` separates
all prepared-body calls from `fixed_body_calls` that match this guard.

A source function receives authenticated ownership before exposure, but no
required-type-boundary marker. Code/default/closure metadata freezing is a
separate adoption operation required for module/class integrity. Unadopted
functions remain instrumentable; an already frozen function cannot lose its
seal through class decline. All functions retain ordinary annotation semantics.

`strict_field_bindings` owns actual lexical targets for selected field writes;
`strict_nominal` reuses callback-free capture readers and resolves targets for
independently guarded field/method requests. Function-owned target selection
comes from the same eligible field/method sites as those requests, not from
the set of annotated parameters or returns. Required fields retain their own
declaration-bound targets; direct-Self fields bind once to the selected final
type before instance admission. Optional capability publication may decline
when its actual target is unavailable. It cannot reject an ordinary call or
grant parameter/return-type proofs. Class-dictionary operands still require
the exact original namespace execution and live native owner; copied
dictionaries, matching source names and recycled addresses confer no authority.

`soac_jit_make_function_with_closure(function_id, kind, captures, defaults,
annotation_provider, globals, caller_environment)` receives the actual active
ABI environment explicitly. A strict class-namespace activation can propagate
its consumed, Rust-only execution identity into the new function owner. A
Python-supplied function ID or a method's own historical creation identity
cannot activate this path. The forced entry interpreter passes the same active
identity through its explicit shared-state constructor.

`soac_jit_complete_function_definition(function_id, function, globals)` is the
private ABI for the explicit `CompleteFunctionDefinition` IR operation. It
borrows the actual function and globals and returns an owned alias. The
lowerer emits it only at recorded undecorated source-definition sites, after
defaults, annotation-provider and type-parameter setup and before the source
binding. No Python helper name or user-supplied integer can create such a site.
The runtime authenticates the actual owner, compiler catalogue/template,
function ID and module execution. Initializing modules defer adoption to their
sealing phase; late free definitions in an already sealed execution finalize
there. Lexical class-owned functions remain the actual class's responsibility,
including overwritten definitions absent from its final member catalogue.
`ModuleTypeFacts::source_class_owner` provides only that static classification;
assigning an independent function into a class does not transfer its ownership.
Arbitrary decorators do not receive a completion operation or a retained
original-function ticket. Unresolved required nominal targets still fail closed;
this operation does not introduce a forward-binding or lazy-rebinding protocol.

`soac_jit_construct_class(site_id, function_env, name, namespace_function, bases, keywords,
requires_class_cell, requires_class_dict_cell, first_line, decorator_preparation,
globals)` is the resolved strict `ConstructClass` operation's ABI. It borrows
seven Python operands, an optional private decorator preparation (null if absent),
and actual globals and returns an owned result or null with the original
exception. The compiler-passed `function_env` must identify the active
authenticated class-construction frame. Its captured owner, source role,
runtime function identity, actual module execution, and globals must agree
with the namespace function's authenticated owner. Neither the numeric site
nor a same-source namespace function alone is a Python-facing capability.
The existing absent-keywords `None` operand is normalized to an exact empty
dictionary before native preparation. The namespace call receives the actual
prepared namespace and a GC-visible single-use handle bound to that namespace,
its actual function, and module execution. A namespace that forwards private lexical bindings also
receives the original cells in the exact authenticated template projection;
an absent capture cannot satisfy a nonempty projection. Binding consumes the handle before body execution,
moves the private cells into the active environment, and clears every handle
edge. Handle teardown also clears edges when argument binding or execution
fails, even if ordinary code retained the handle. Subsequent admission retains
only a Rust execution identity. Both source backends use native ABI4 Pending
construction before Ready callbacks, with the actual source-requested layout.
Pending blocks allocation and assignment as an instance's new `__class__`;
source ownership is already active, but direct-Self and own field targets await
the selected final type. The plain actual construct result,
or the authenticated dataclass Apply result, completes mandatory admission
before publication, even while its module initializes. Native policy and seal
publish before the barrier opens. The module's weak, one-target-at-a-time drain
still completes module nominal leaves and optional retained capabilities.
Pre-construction dynamic decline receives no source type handle or layout authority;
it does not revoke any already-installed metadata or inherited contract.
`PySoac_PrepareClass(name, bases, keywords)` returns the metaclass, namespace,
resolved bases, and keywords; it does not allocate class cells. The native
compiler's source-bound class recipe selects the body's ordered cell
initialization, current raw slots, closure captures, and exports. The body
returns its actual current class cell or `None`, rather than an independently
preallocated cell or a tuple of pinned historical cells.
`PySoac_CompleteClassNamespace(preparation, original_bases)` only completes
`__orig_bases__`; the body writes native cell exports in their recorded order.
`PySoac_FinishClass(name, returned_class_cell, cls)` checks the cell actually
returned by that body. Native `type.__new__` handles the namespace's exported
class-dictionary cell and its copied type dictionary. The
long-lived namespace execution identity remains Rust-only and `Send + Sync`;
it does not retain the namespace, module wrapper, or any historical Python cell.
The class policy owner holds no intrinsic strong reference to its actual type
or bases. A reserved GC edge receives the original type's callback-free weakref
once, before class callbacks. This exact builtin weakref allocation only
schedules GC in the pinned GIL runtime; binding rechecks the native owner and
prepared phase before committing it. `for_constructed_type` authenticates
Pending/Admitting ownership for construction and selected-type nominal binding;
it is not receiver or optimization authority. `for_actual_type` requires the
actual permanent contract and original weak witness. A recorded address plus
an exposed owner reattached to
a new native type cannot recover that view, even after address reuse. Native
type teardown terminalizes its contract before releasing references; a retained
Rust owner also rejects its expired type witness. An escaped class namespace
therefore does not keep the class alive or prevent ordinary namespace clearing.
Class-dictionary execution coordinates are only candidates for
`PyDict_MatchesSoacClassNamespace(dict, expected_owner)`: it checks the actual
dictionary's private native policy role, completed installation, and live
type/owner binding without dereferencing the supplied owner address. A Rust
consumer must additionally check its private owner type, original actual-type
weak witness, and exact namespace execution Arc, with no intervening Python
effect, to exclude exposed-owner replay and address reuse.
Neither check turns a pending or merely native-sealed class into an optional
retained capability.

The current construction helpers have distinct responsibilities:

| Helper | Responsibility |
| --- | --- |
| `strict_class_state::bind_pending_type` | Registers the write-once storage-state factory under the unopened Pending barrier, records the actual namespace and weak type witness, and seals fresh explicit descriptors before callbacks; no provisional Self binding or permanent type payload. |
| `strict_class_state::commit_pending_type` | Revalidates the captured final contract and actual member offsets while admission is closed. |
| `strict_class::admit_class` / `strict_class::finalize_class` | Complete mandatory selected-result admission / additionally publish eligible module-final retained capabilities. |
| `PyType_GetSoacConstructionInfoV1` | Reports the actual single native construction state, including Failed with a permanent payload. |
| `PyType_AdmitSoacPendingV1` | Uses the originally captured commit hook and publishes the permanent policy plus native seal before Enforced. |
| `PyType_FailSoacPendingV1` / `PyType_DisposeSoacProvisionalV1` | Close a failed lineage / dispose only an unselected provisional with no permanent contract under the resolved selected lineage. |

Native ENFORCED construction remains a separate C kernel; neither source
backend selects an ENFORCED Rust binding fallback. ABI4 dictionary mode is
explicit: NONE, INDEXED, or ORDINARY. Source dictionary-bearing types select
ORDINARY, preserving actual dictionary identity and ordinary inline layout.
Supported fresh allocations use the optional storage-state path below;
`prepare_instance_dictionary_policy` remains the enforcement path for existing
or unsupported storage. `new_instance_dict` remains the separate INDEXED kernel.
Admission never flips `INLINE_VALUES` or grants an indexed source-field capability.

The optional storage-state boundary has these responsibilities:

| Helper | Responsibility |
| --- | --- |
| `PyType_SetSoacStorageStateFactoryV1` | Registers one trusted factory during the original unopened Pending bind. Registration neither admits instances nor replaces class authority, and cannot be replayed later with an exposed owner. |
| `strict_class_state::prepare_storage_state` | Prepares effective already-bound rules from the actual admitted type and declaring owners, retains the actual MRO across callbacks, and revalidates it before transferring one owned native state to the allocator. It receives no instance. |
| `dictionary_storage_projection` / `member_storage_projection` | Select dictionary-only obligations or one actual declaring member's predicates. Equal field spellings do not merge distinct nominal bindings or physical slots. |
| `StrictFieldChecks::project_fields` | Retains only the selected bound predicates and their necessary nominal targets; a dictionary projection does not retain unrelated slot targets through its original owner. |
| `PyTypeState_NewV1` | Checks the exact ABI and complete actual native owner/index/name/offset catalogue, copies slot rows, and creates GC-visible instance state with a separate dictionary projection. Constructor inputs are not storage authority by themselves. |
| `PyObject_GetTypeState` | Returns a borrowed live state through the checked allocation-family/presence-bit accessor. An ordinary unmarked object has no state; a marked terminal or malformed attachment fails closed. |

Native allocation caches immutable state in the existing GC-owned `tp_cache`
field of the actual participating heap type. Type identity, liveness and version
receipts are checked at allocation; callback-capable preparation also retains
and rechecks the actual MRO and factory. A foreign non-null cache is never
overwritten. This does not add callback-capable cache invalidation to
`PyType_Modified`, grow ordinary types, or discover a receiver/MRO on each write.

Fresh instance allocation reserves and initializes its trailer before reference
tracing, callbacks or traversal can observe it. Inline storage uses the prepared
rules before materialization. A newly materialized exact dictionary acquires
only the dictionary state, with no receiver or unrelated slot-owner backedge.
It retains actual nominal targets needed by its own predicates. Published rules
are immutable; installation, mutation and terminal attachment flags belong to
each storage object, so clearing one dictionary cannot revoke a sibling's policy.

The initial native protocol supports the audited 64-bit little-endian GIL
layout; Linux AArch64 is the tested platform. Default fixed-size GC heap
instances with the audited allocator/traversal/free family and an `object` solid
base, plus their freshly materialized exact dictionaries, participate. Ordinary
allocations reserve no state pointer; stateful dictionaries bypass the ordinary
dictionary freelist. Pre-existing/replacement dictionaries, custom or variable-
size allocation, and mixed legacy/factory families retain their existing
enforcement rather than gaining a copied or retagged trailer. Unversionable
types likewise remain on the legacy path. These paths are not claimed as
completed direct-state migration.

The existing `soac_jit_runtime` reference helpers update only the low `u32`
reference count, preserving the adjacent overflow and object-flag fields,
including the optional-allocation marker. A nonzero layout flag must not hide
the last-reference deallocation or alter immortality tests. Structured tests
validate this ABI requirement; the promoted packet does not change the raw
helpers or add a helper family or optimization claim.

The native and Rust source changes are promoted together. The fresh optimized
build, matching extension and real-checker integration matrix, and full project
gate remain pending at the 2026-08-25 (PDT) source-promotion checkpoint.

`strict_descriptor.rs` owns the single-builtin source descriptor boundary:

- `soac_jit_apply_function_descriptor(site_id, function_env, decorator,
  original_function, frame_namespace)` receives the original compiler-recorded
  function creation operand, never the result of an intervening decorator. A
  signed single builtin proposal, actual immutable factory, authenticated source
  function/code/owner, and active namespace execution select native
  `PySoac_NewBuiltinDescriptor`. A rebound factory receives one ordinary
  contextual call with the original function. Caller operand cleanup is shared
  with ordinary calls; no preparation object retains a Python function root.
- `matches_birth` validates the actual descriptor's opaque native birth and
  Rust-only namespace execution, including current component/owner/code and
  the non-reused native birth ID recorded once by the original producer. An
  exposed owner cannot authorize a new C-API birth. A
  foreign optional owner declines; a recognized terminal strict owner errors.
- `adopt` consumes that proof for the actual constructed class via
  `PySoac_AdoptBuiltinDescriptor`, after complete namespace validation in the
  Pending bind, before class callbacks. Implicit source methods remain plain
  functions until native type construction creates and seals their wrappers.
  Final selected-type admission revalidates those same seals before instances.
  Component ownership checks precede callbacks;
  descriptor adoption permanently seals metadata and is never revoked. Getter
  properties acquire no dictionary field or function-result type check.

`strict_class_decorator.rs` owns the explicit source-selected decorator fallback:

- `soac_jit_prepare_class_decorator(site_id, function_env, factory, callable,
  argv, positional_count, keyword_names, frame_namespace)` receives the already
  evaluated callable and raw arguments, checks the actual active frame's
  captured owner, signed class proposal, construction template, module
  execution, and globals, and then invokes a factory once. Bare decorators
  are not called. The caller's ordinary argument roots and reverse cleanup
  remain in the shared call path; neither the factory nor its arguments enter
  the returned carrier. The registered dataclass selector can authenticate an
  actual helper graph and create its explicit native invocation before wrapper
  creation/CREATE watchers. An unknown graph uses the ordinary once-only call.
  Neither path creates the namespace function early.
- `soac_jit_prepare_class_decorator_unpacked(...)` borrows the existing
  `CALL_FUNCTION_EX` positional tuple and keyword dictionary into that same
  raw boundary. It creates no additional Python argument container. Plain
  positional/named calls do not use it; their only tuple contains keyword names.
- `soac_jit_apply_class_decorator(site_id, function_env, preparation, class,
  frame_namespace)` consumes exactly the recorded construction result and calls
  the same decorator once, using its actual bound native invocation when one
  was admitted or `PySoac_VectorcallWithContext` for ordinary decline. Successful
  Apply completes the actual generated-member proof and admits the selected
  result before source publication, including during module initialization.
  A slots replacement has a distinct linked Pending construction; only the
  selected type receives the permanent contract. Exact native lineage permits
  disposal of the unselected original, never revocation. The ordinary
  callable edge transfers out of the carrier during the call and back afterward
  so class-argument cleanup precedes decorator cleanup on success and error.
- `soac_jit_discard_class_decorator(preparation)` clears that edge in the
  compiler's explicit `finally` region, even if the carrier escaped. It marks
  completion/failure before releasing Python references, closes the temporary
  native invocation, and releases its catalog/class/builder edges. Failed
  application attempts removal of only its exact source/actual-object weak
  record even if the terminal native-owner query fails after permanent
  publication. Cleanup preserves the application error and installed protection,
  so a caught failure cannot poison an unrelated later drain. A quiet binding
  deletion handles locals and suspended-frame storage alike; internal
  `await`/`yield from` control flow does not discard a still-needed preparation.
  The source class binding occurs only after this cleanup.

Generated constructors have no parameter, return, `InitVar` or default-factory
result checks. Selected field writes reuse their actual declaring field owners;
pending slots replacement shares unresolved own-Self field targets, bound only
at final selected-type admission. Inherited field owners keep their actual
declaring targets. Missing required field operands fail closed before admission;
annotation caches and later cell writes cannot retarget an installed policy.
The dataclass callback table uses ABI 4 with `enter`, `create`,
`validate_member`, `bridge`, `compiled`, `created`, `validate_component` and
`prepare_slots`; the type-check-only callback and deferred-value bridge are
removed. The actual callable graph, permanent type policy and optional guarded
capabilities remain distinct from function call semantics.
Generated functions retain the structural closure-completeness guard before
entry and `PyFunction_AdoptSoacDataclassComponent` ownership for annotation/repr
components; neither validates argument or result values.

The selected native dataclass member kernel supplies
`PyType_SetSoacDataclassMember(invocation, actual_type, name, function)`.
Its opaque member operation consumes one fresh creation record and carries
explicit provenance through the ordinary type dictionary/version/slot path.
Registered validation precedes key resolution; the once-resolved commit uses
callback-free native revalidation after the last watcher. A Pending write
authenticates the exact construction and inherited-only policy, not a full
permanent namespace grant. Publication records the actual member birth at the
dictionary effect before displaced-value release. Ordinary mapping writes gain
no permission; terminal, inherited-final, and field-descriptor barriers remain.
Native kernel and genuine source/checker tests are separate validation gates.

On a failed native Apply, `strict_interpreter::completion::forget_failed_dataclass`
removes only weak class receipts matching the actual caller invocation, source,
native Failed owner, original type witness, and dataclass graph edge. Generic
construction queries still refuse Failed authority. Cleanup neither drains
unrelated records nor removes an escaped type's allocation barrier, and the
original body/completion error retains its ordinary caller continuation.

`strict_annotation.rs` supplies four mechanical annotation operations:

- `new_annotation_set()` allocates an exact set with `PySet_New`.
- `setup_annotations(namespace)` runs native `SETUP_ANNOTATIONS` semantics on
  the explicitly selected module globals or actual prepared class mapping. It
  preserves an existing entry and the mapping's ordinary lookup/store behavior;
  it does not infer the namespace from the caller's native frame.
- `record_annotation(indices, index)` records a reached conditional annotation
  with `PySet_Add`, after its ordinary assignment has completed. It does not
  dispatch through a mutable Python `.add` attribute.
- `check_annotation_format(format)` performs the native comparison with
  `VALUE_WITH_FAKE_GLOBALS` and raises the canonical native
  `NotImplementedError` for a greater format. Rebinding Python exception names
  cannot change this annotationlib protocol check.

The corresponding public core nodes are `NewAnnotationSet`, `SetupAnnotations`,
`RecordAnnotation`, and `CheckAnnotationFormat`; JIT and forced-entry consumers
use the same operations and preserve exceptions while releasing operands.
These operations grant no execution or optimization authority.

Lazy type expressions use three explicit public core operations:
`CreateTypeAlias`, `CreateTypeParameter`, and `SetTypeParameterDefault`, with
`TypeParameterKind` selecting the native parameter form. Their matching Rust
helpers in `strict_annotation.rs` authenticate the actual evaluator's native
owner, expected function ID, source role, code, capture projection, and globals
before calling the native factory. After allocation they recheck that identity
and `PySoac_MatchesTypeExpression`'s actual private evaluator slot. They do not
evaluate the expression, seal the target, retain it in a registry, or give an
escaped evaluator immutable authority. Parameter defaults attach in a separate
operation after parameter creation. `AnnotationProviderKind` distinguishes a
dictionary provider from alias-value, bound, constraints, and default
evaluators. Type evaluators retain the native positional-only `.format` code
parameter with default `1`; this pinned CPython's `inspect.signature` rejects
that invalid Python parameter name, and SOAC does not silently rename it. The
same-root native catalogue also checks each evaluator's explicit original
source span, distinguishing bounds/defaults with equal names on one line.
Generic declarations additionally use the public core operations
`ConstructTypeParameterScope`, `SubscriptGeneric`, and
`SetFunctionTypeParameters`. Their mechanical runtime helpers are
`construct_type_parameter_scope(expected_function, positional_defaults,
keyword_defaults, actual_scope_function, globals)`,
`subscript_generic(type_parameters)`, and
`set_function_type_parameters(expected_function, function, type_parameters,
globals)`. Scope construction evaluates enclosing positional/keyword default
containers before creating and calling the actual hidden function through its
authenticated SOAC entry. `CallableSourceRole::TypeParameterScope` and
`TypeParameterScope` record the original declaration, native signature and
capture projection; this role requires an individually matched original code
object and cannot derive admission from a generated helper name.
`TypeParameterScopeInput`/`TypeParameterScopeInputKind` describe the native
`.defaults`/`.kwdefaults` inputs separately from their private body bindings.
`FunctionDefaultsProjection::NativeContainers` preserves the already evaluated
tuple/dictionary rather than rebuilding them or scanning their keys. Defaults
are never inferred from arbitrary Python container shapes. Native Generic
construction happens before explicit class bases/keywords, and function
type-parameter metadata is attached before decorators/completion/source binding.
The type tuple retains the originally created parameters, rather than
re-reading potentially changed parameter cells. A starred TypeVarTuple default
uses the ordinary single-element unpack operation in its lazy evaluator.
Type scopes and type-expression evaluators remain provenance-only and do not
allocate weak pending-adoption records; dictionary providers, source functions,
and class namespace helpers retain their existing adoption responsibilities.
This generic producer is implemented but its genuine runtime matrix remains
pending the coordinated native-helper generation and extension checkpoint.

`initialize_strict_runtime(py)` installs the exact Rust replay resolver in the
native per-interpreter SOAC state at extension initialization. The resolver
authenticates the actual provider, its privately matched original code, and its
native closure projection before asking `PySoac_CloneAnnotationReplayCode` for
an ordinary recursive clone. It rechecks owner/code/closure identity after code
allocation callbacks. The clone has no strict flags or private source IDs and
is never an optimizer capability. It grants no original-code entry authority:
retained SOAC entries use their authenticated trampoline, and original CPython
frames require the interpreter backend's checked activation. `annotationlib`'s
contextual `owner`, including
`None`, does not grant or replace this authority.
`CellCaptureProjection` explicitly distinguishes taking a lexical cell
reference from passing an already-owned cell object, while
`AnnotationProviderScope` separates the public one-argument `format` signature
from its private body binding and records a class dictionary projection.
`CallableScopeInfo.cell_value_aliases` gives compiler-selected implicit cell
loads separate body bindings without creating additional closure cells. A
source reference with the same native logical name still follows its ordinary
class-dictionary/lexical lookup policy. These are serialized compiler decisions,
not checks for helper-like names.
Strict module or class lazy annotations in a `finally` suite currently fail
explicitly: CPython can emit that suite several times with distinct annotation
indices, and source-order approximation is not a supported substitute for
native occurrence provenance. Function-local annotations and unrelated
`finally` suites are not rejected by this limitation. With future annotations,
the native compiler instead creates eager string dictionaries in module and
class scopes; these have no conditional-index replay restriction. Function
providers retain the native one-argument signature but have no lexical or
class-dictionary captures for stringized annotations.

`CompiledStrictSource::compile` owns the authenticated native root across
lowering and supplies `CanonicalAnnotationStrings` from that same native parse.
The latter is source-bound semantic data, not an authority token. After
lowering, consuming `CompiledStrictSource::into_function_catalog` matches the
same native root to the explicit source and provider plans. Strict future
annotations require exact native expression-range entries rather than using a
second parser's pretty-printer to approximate Python's annotation strings.

`PySoac_VectorcallWithContext(callable, args, nargsf, kwnames, globals, locals, builtins)`
and `PySoac_ObjectCallWithContext(callable, args_tuple, kwargs, globals, locals, builtins)`
are native call-boundary APIs. They preserve ordinary calls and invalid-argument
errors, but recognize the actual native builtin method definitions for
`locals`, zero-argument `vars`/`dir`, and `globals`. Zero-argument `dir` sorts the
actual local namespace's keys; object-argument calls keep their ordinary
protocol. Class calls supply the resolved `Call.frame_namespace` operand;
function scopes without a materialized local namespace reject its inspection
explicitly rather than observing an unrelated native frame. Globals are the
actual caller environment; the borrowed builtins pointer is pinned by that
environment for the call. No callable wrapper, Python attribute, thread-local
context or reconstructed frame transfers authority.

Canonical `compile` uses ordinary argument binding and conversion, with
`dont_inherit=True` required on this frame-free path. Canonical `eval`/`exec`
accept an ordinary code object and explicit globals, preserving mapping and
closure validation, audit callbacks, exception behavior and installed write
barriers. They insert captured builtins only when the target globals lacks
`__builtins__`. The shared evaluator reloads that entry after audit callbacks;
if a callback deletes it, the already captured mapping supplies the fallback
without reinsertion or a caller-frame lookup. Ordinary CPython wrappers keep
their existing caller-context behavior. Inherited source strings and execution
authority for strict dynamic code still require a separately specified protocol;
their refusal does not add frame-inspection or observer guarantees.

Plain positional and named-keyword calls use a raw native argument vector in
both compiled and entry/deopt execution. Only keyword names enter a tuple;
argument values are supported by the call's owned/borrowed inputs.
Evaluation captures the callable before all argument expressions and preserves
it across callbacks. On the borrowed call path, cleanup releases names,
reverse-order operands, then callable on success or error. A later argument
failure releases earlier owned values and retains the original exception.

For a generic positional call whose callable is an actual `TakeOperand` and
whose arguments are distinct `TakeOperand` moves or fresh call results,
`dp_jit_call_owned_operands` takes those existing owned expression references
through the ordinary contextual vectorcall boundary. This
selection requires the resolved physical Operand ownership layout in both
compiled and entry/deopt execution; native frame correspondence is irrelevant.
Ordinary local/cell/ABI loads, keywords/unpacking, explicit frame namespaces,
and typed call access plans other than Generic retain their existing paths.
There is no reference-token ABI or inferred native borrow ancestry.

The shared IR predicate validates that exact physical input-ownership shape.
Late expression linearization preserves the complete selected call inside its
source activation: its child evaluation order, semantic IDs and source ranges
stay attached to the call. Acquired operands retain explicit ownership through
fallible later evaluation and suspension. Other calls retain their existing
linearization and selection rules. No ordinary local is made eligible by its
name or liveness. Extra transient owners are permitted; this representation
serves correct transport and cleanup, not a CPython reference-count target.

The captured environment supplies caller context without reconstructing a
Python frame or imposing an observer-compatibility gate. Ordinary Python
functions, strict source-owned entries and other callable shapes all use their
actual public call protocol; no SourceEntry registration or native interpreter
birth is fabricated. A failed call is never retried.

The helper consumes the raw transport array on every valid-input return,
publishes NULL before each release and preserves the primary exception. It
returns one owned result or an error. No token conversion, variable-size token
scratch buffer, consuming native-frame binder or separate synchronous body
interval remains. Source-local storage and safe cleanup use the shared
source-owned entry path without SOAC traceback reconstruction.

### Source-owned SOAC body execution

Compiled and entry-interpreter functions use the existing source-owned vectorcall
boundary and ordinary owned `PyObject *` storage. The boundary authenticates
the actual source owner, captures invocation identity, binds arguments once,
and preserves ordinary parameter/result behavior. Cleanup preserves a pending
exception and retires all owned values. An allowed code/vectorcall change does
not retarget an invocation already in progress or remove installed metadata
protections. `PySoac_SetInterpreterCallbacksV2` installs
`PySoacInterpreterCallbacksV2` (callback ABI 2): `root_begin`, `root_end`,
`birth`, `function_attribute`, `enter`, `started`, `call`,
`selected_call_finished`, `returned`, `failed`, `leave`, `prepare_type`, and
`definition_store`. `birth(parent, function, new_owner)` has no required-type
output; `enter(kind, subject_owner, frame, parent, new_call_state)` has no
type-check snapshot, and there is no bound-argument type-check callback.
Frame and call views remain V1. `returned` still completes pending child
definitions on successful original synchronous execution; it does not check
the borrowed result's type. `failed` preserves the original body exception
while completing or terminalizing still-pending definitions.
The invocation captures Rust-owned template, module, compiler and compiled-body
handles before argument-binding callbacks. Its environment and source owner
then keep the actual Python identities alive. No private metadata pointer or
borrow survives a callback that may replace the function's opaque association;
later metadata writeback revalidates that association. The entry interpreter
uses the captured template and layout instead of rereading mutable metadata.

The separate `native_source_execution` / opaque-token body path and its
`soac_jit_native_ref_*` helpers were removed under the 2026-08-24 (PDT)
execution-compatibility clarification. SOAC does not reconstruct CPython's
borrow/duplicate/fused-opcode schedule. The 2026-08-25 (PDT) amendment also
removes SOAC traceback/frame reconstruction and projection helpers. Ordinary
exception state, source authentication and safe cleanup remain. Generic field
and call observations do not grant layout, unchecked-target or check-elimination
authority.

Calls with unpacking retain their separate mapping path. The entry/deopt path
uses native `_PyStack_UnpackDict` for key validation and keyword identity. That
native stack borrows positional operands and owns only appended keyword values
and its names tuple; cleanup releases each ownership exactly once.

The exported `dp_jit_match_sealed_field_capability(receiver, capability)` in
`strict_class_state.rs` authenticates the private, runtime-only sealed-field
capability for the raw kernel above. Its result is `1` for a matching live
receiver, `0` for generic fallback, or `-1` with the original exception set; an
error must not be retried as getattr. The caller pins the Rust capability while
machine code uses it. The capability keeps the actual namespace-execution Arc,
not a Python class/default/namespace reference, and compares actual native
owner/type identity and the original type's weak witness before using recorded
addresses. Source names and profile
offsets cannot create it. Actual dictionary policy identity is checked through
the native predicate; an active write transaction conservatively falls back so
its callbacks can still perform ordinary reads. Typed-plan consumers must bind
this capability explicitly; exporting the kernel alone does not select an
optimization or authorize a raw store. Source ORDINARY dictionaries do not
publish its indexed variant. Actual source-requested native object slots may
publish only after the separate retained class/module-final capability checks;
the CPython interpreter backend publishes no JIT capability.

The capability begins with a `repr(C)` `RawSealedFieldLayout` containing the
actual expected type address, storage kind, logical field index, actual native
member offset, and the two class-default locator indices (`-1, -1` for no class
binding). Its owning Rust type publishes
the offsets with `offset_of!`; codegen must not duplicate guessed offsets or
read this descriptor before the capability match. The descriptor adds no
Python references and is not a serializable source/profile capability. Actual
function activations must pin their immutable capability-slot snapshot rather
than rebinding shared compiled code to the most recent class-factory execution.

`dp_jit_resolve_sealed_method_capability(receiver, capability, callee_out)` in
`strict_class_state.rs` resolves only a protected plain instance method of an
exact actual sealed class. Publication authenticates the final MRO binding,
native function owner/seal, source identity, and the declaring class's actual
namespace execution. It also requires final module bindings and no pending
module nominal leaves on the actual declaring function, including inheritance;
early mandatory method sealing alone is insufficient. Resolution returns `1`
with a borrowed callee, `0` for
ordinary lookup, or `-1` with an exception; the caller owns the receiver and must
INCREF the callee before effects or operand cleanup. The capability retains no
class, function, globals, default, or namespace Python edge. Protected native
class precedence makes instance-dictionary aliases irrelevant to this method
lookup, but ordinary subclasses still miss. This grants lookup/binding
elimination only: arguments are evaluated normally and the actual function is
called through its checked public trampoline. Default liveness, binding errors,
and required checks must not move from the call into preceding method lookup.
Explicit wrappers/properties and unchecked direct-body entry are not covered.

`dp_jit_resolve_sealed_virtual_method_capability(receiver, capability, target_out)`
extends that lookup kernel to actual sealed descendants. Each sealed class
publishes immutable rows for the exact ancestor construction families in its
MRO; a row stores receiver-specific authenticated implementations, not copied
base-class exact-receiver capabilities. Canonical slots belong to an immutable
Rust family Arc. Ordinary subclasses, unrelated same-source factory executions,
and legitimate shadowable-field overrides miss. Source/digest requests bind
only within the actual class's published MRO; ambiguous same-source ancestors
do not choose a family by address or spelling. Rows and capabilities own no
Python type, function, namespace, or default reference.

The virtual resolver returns `1` with a borrowed `RawSealedMethodTarget`
(`callee`, `entry`), `0` for normal lookup, or `-1` with an exception. The entry
comes from authenticated private SOAC metadata, never from the public mutable
function `vectorcall` pointer. Resolution neither compiles nor reads/checks
defaults. The caller must INCREF the captured callee before argument evaluation,
then compare its current public vectorcall with the selected entry immediately
before invocation. On a changed pointer it invokes normal dispatch on the
**same captured callee**, without repeating attribute lookup. This preserves
supported C setter changes during argument evaluation. The private checked
entry still performs normal per-call activation, binding and default-liveness
validation, with ordinary result/exception behavior. No argument/result type
proof is granted.

`soac_jit_resume_generator` is the direct five-argument C ABI for the
compiler-owned `resume_generator` runtime primitive. It borrows its resume
function, generator owner, preserved state, send value, and exception value;
it returns an owned Python object or null with the current Python exception
set. It is selected only for the resolved runtime primitive, not for a
same-named Python global.

Strict generator/coroutine/async-generator calls bind source parameters once
at object creation. Creation pins immutable compiler metadata before binding;
no borrow of mutable function metadata spans keyword/default callbacks that
can reenter the same generator function or replace its code. The already-bound
generator retains its original code and cells, while subsequent calls see the
replacement. Checked source creation constructs its capsule and native object
directly; it does not pass source authority through a Python factory helper.
The compiler-intrinsic `make_generator_instance` takes its eight ordinary
factory inputs plus explicit ordered `operand_slots` as its ninth argument.
`make_preserved_state(initial_values, slot_kinds, operand_slots)` validates the
unique in-range boxed operand indices and initializes their None placeholders
as NULL. The direct Rust builder uses the identical roles without creating
Python tuples. Terminal capsule cleanup clears operands in reverse acquisition
order before other local cleanup, publishes NULL
before each release, rejects resume/replacement, and tolerates recursive clear.
A strict generator expression supplies its uniquely matched original native
code from the separate code-exposure catalogue. That input controls
`gi_code/ag_code` and native names only: it does not replace the compiler helper's
execution code, closure ABI, or rooted creation/admission witness. The parser's
explicit expression/iterable projection distinguishes same-line occurrences;
missing or ambiguous native code is an explicit error, not a name-based fallback.
The verified generator-expression projection also selects denial-only native
bootstrap code before helper CREATE. The current rooted template and separate
code-exposure entry must agree; a kind or name alone is insufficient. The clone
retains source ID zero and rejects native entry before owner/closure installation,
without changing the original code exposed by the completed generator.
The native preserved-state capsule traverses an immutable suspended-frame owner
with the original source/function identity, globals, builtins, and closure-cell
identities. Resume validates that exact capsule and generator-like lowered body,
then uses an owned per-resume environment. It never checks the internal control
operands or yielded values against a synchronous source signature. Completion
releases the suspended cells before returning to the caller; GC terminalizes
the state before any decref can reenter. A different capsule, function, or
cleared owner cannot authorize a strict resume.

## Synthetic Inter-Pass Markers

These helper-shaped names are compiler-internal markers. They may appear in
intermediate AST or BlockPy during lowering, but a later pass must replace them
with structured IR or dataflow before runtime execution.

```text
current_exception
```

- `current_exception()` is emitted by exception/with statement lowering to mean
  "the active exception object for this exception edge". Name binding rewrites it
  to the block's explicit exception parameter before codegen. It is not
  `soac.runtime.current_exception` and should not be exposed as a Python helper.

## soac.bootstrap

`soac.bootstrap` is evaluated through normal Python, not the SOAC import
transform. It provides constants and function-instantiation helpers needed while
`soac.runtime` itself is still initializing.

`code_with_freevars` compiles inert closure-shaped entry code using private
source placeholders, then preserves the requested capture names and order in
`co_freevars`. The native generic-class cell name `.type_params` is supported as
metadata without interpolating it into Python source. Other invalid source
names remain rejected. This helper does not authenticate native source code or
grant execution authority; executing its placeholder body still raises.

Top-level functions defined by `soac.bootstrap`:

```text
code_with_freevars
_entry_template
```

## soac.runtime

`_reraise_control_flow` clears its exception argument in a `finally` clause so
an escaped exception's traceback cannot retain that helper argument. This does
not replace or clear the native caller's handled-exception state.

`_reraise_after_generator_step` releases a terminal generator or async-generator
wrapper's reference to its actual resume function after the wrapper has left
its exception handler. It preserves the exception's original context and the
wrapper's code/closed metadata. Clearing that wrapper edge does not clear the
source function's lexical cells: another live function reference or suspended
frame continues to retain them. Coroutine wrappers use the same generator
path. The post-handler release keeps finalizers in the surrounding caller's
handled-exception context instead of the transport exception's handler.

Top-level functions defined or re-exported by `soac.runtime`:

```text
_unsupported_frame_builtin
_index
IterRange
tuple_from_iter
list_from_iter
set_from_iter
map_from_iter
__soac_map_iterator
filter_from_iter
__soac_filter_iterator
constructor_call
__deepcopy__
templatelib_Template
templatelib_Interpolation
bb_trace_enter
_current_yieldfrom
_is_cancelled_error
_is_generator_closed
_reraise_after_generator_step
_reraise_control_flow
resume_generator
resume_async_generator
generator_resume_delivery
inject_generator_resume_exception
async_gen_wrap_yield
make_preserved_state
load_preserved_state
suspended_handled_exception
_normalize_throw_exc
_current_throw_context
make_generator_instance
complex_from_parts
class_lookup_cell
class_lookup_global
_validate_exception_type
exception_matches
exceptiongroup_split
unpack_fixed
unpack
call_super
call_super_noargs
_match_class_validate_arity
match_class_attr_exists
match_class_attr_value
code_template_gen
code_template_async_gen
annotation_forwardref_value
create_class
exc_info
exc_info_from_exception
_get_awaitable_iter
await_iter
raise_from
pep_479_exception
_call_exception_class
import_
import_attr
import_star
_lookup_special_method
_has_special_method
_missing_context_protocol_message
contextmanager_enter
contextmanager_get_exit
contextmanager_exit
_ensure_awaitable
asynccontextmanager_aenter
asynccontextmanager_get_aexit
asynccontextmanager_exit
```

`suspended_handled_exception(capsule)` returns a new reference to the actual
owned suspended exception item, or `None` for an empty, unstarted, or closed
execution. It does not consult the caller's current item or retain an extra
snapshot. `_current_throw_context` uses this read-only projection; generator
factories and wrappers no longer carry a throw-context slot index. Supported
C-API replacement therefore changes the sole authoritative item, without a
stale saved field retaining its predecessor.

`generator_resume_delivery(capsule)` reads the compiler-owned exceptional
resume decision. It grants no permit. `inject_generator_resume_exception`
consumes the binding's one owned normalized error into its actual handled
item; lowering first retires the control parameter and uses an Operand-lifetime
temporary. Ordinary source raises remain separate. `async_gen_wrap_yield(value)`
allocates an exact native async-yield token at the source operation, so a failed
allocation enters the source handler before the activation suspends.

`pep_479_exception(kind, cause)` distinguishes generator (0), coroutine (1),
and async-generator (2) source exceptions. Kind 2 converts both StopIteration
and StopAsyncIteration; explicit `GeneratorReturn` is not a source exception.
The managed-native consumer and these helpers require the new native ABI and a
matching extension/support build; their runtime validation is still pending.

`class_lookup_global(class_ns, name, globals_dict)` performs only the named
namespace lookup followed by globals and builtins. It never scans unrelated
members or their `__type_params__`; lexical generic parameters are passed to
`class_lookup_cell` by the resolved binding plan. This preserves lookup effects
and prevents lazy framework members from being triggered by other annotations.

Iterator-pipeline helper roles:

- `list_from_iter` and `tuple_from_iter` are the production inline bodies for
  exact builtin `list` and `tuple` consumers when typed planning proves a
  single-use closed generator-expression pipeline. The resulting collection
  remains a real Python object.
- `map_from_iter` and `filter_from_iter` eagerly acquire the source iterator and
  return the compiler-owned `__soac_map_iterator` and
  `__soac_filter_iterator` workers. Production may inline those workers and
  their generator-expression producer into a selected list/tuple materializer
  region.
- `set_from_iter` is a callable runtime helper with ordinary Python behavior,
  but it is not registered as a production builtin-implementation body. Exact
  builtin `set` consumers remain on CPython's native path because expanding
  the Python-level `result.add(item)` loop is not compact or profitable. A
  future fused set materializer should use checked direct `PySet_Add`-shaped
  IR/runtime support instead.

Authenticated source generators retain their public factory, generator object,
and resume activation at the strict `GeneratorResume` boundary. Iterator
specialization must not treat an inferred single-use source generator as
permission to erase that boundary. Ordinary compiler-internal map/filter
calls may still be inlined under their typed eligibility proofs; this does not
imply removal of their generators' resume activations. The retained resume
boundary currently prevents generator-state wrapper erasure.
The current eligibility and introspection boundaries are documented in
`doc/SPECIALIZATION.md`.

Runtime classes and methods:

```text
AsyncGenComplete

ClosureGenerator:
  __init__
  __iter__
  __next__
  send
  throw
  close
  gi_yieldfrom

Coroutine:
  __init__
  __await__
  __iter__
  __next__
  send
  throw
  close
  cr_frame
  cr_running
  cr_code
  cr_await

ClosureAsyncGenerator:
  __init__
  __aiter__
  __anext__
  __getattr__
  gi_yieldfrom
  asend
  athrow
  aclose

AsyncGenSend:
  __init__
  __iter__
  __await__
  __next__
  _step
  send
  throw
  close

_AwaitIterWrapper:
  __init__
  __await__
```

Runtime aliases and re-exports:

```text
next
iter
range
anext
isinstance
getattr
setattr
delattr
tuple
list
dict
set
slice
type
int
classmethod
ascii
repr
str
format
pow
aiter
code_with_freevars
_entry_template
AssertionError
AttributeError
ImportError
TypeError
ValueError

globals = _unsupported_frame_builtin
locals = _unsupported_frame_builtin
eval = _unsupported_frame_builtin
exec = _unsupported_frame_builtin
```

Runtime helpers imported from `soac.sim`:

```text
_MISSING
_mro_getattr
```
