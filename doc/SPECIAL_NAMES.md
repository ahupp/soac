---
title: "SOAC Special Names"
---

# SOAC Special Names

This file is the intended inventory for names that carry compiler or runtime
meaning beyond ordinary Python source names. It is intentionally seeded with
the names involved in the current ownership work; the remaining `_dp_*`,
`__dp*`, and SOAC runtime names still need a full audit.

Special names should have one documented producer, one documented consumer
contract, and a clear answer for whether user source can observe or collide
with them. Code should prefer structured IR facts over prefix tests. When a
prefix test is still used, it should be tied to a generated-name producer and a
specific local/storage invariant.

## `_dp_tmp_*`

Produced by the AST and BlockPy lowering pipeline for temporary local values.
Older AST rewriting uses the process-local `fresh_name("tmp")` shape
`_dp_tmp_<n>`. Function-local BlockPy name generation uses
`FunctionNameGen::next_tmp_name("tmp")`, which includes the runtime module id,
local function id, and temp id:

```text
_dp_tmp_<runtime_module_id>_<local_function_id>_<temp_id>
```

These names are internal locals. They should not be treated as user-visible
storage, but many of them are still real local stack slots because they preserve
CPython-visible cleanup behavior when a later operation can raise.

### Generated-Temp Ownership Transfer

Typed JIT codegen has a narrow ownership transfer for adjacent generated-temp
stores:

```text
Store(target, _dp_tmp_*)
Del(_dp_tmp_*)
```

When the source and delete resolve to the same local location/name, codegen may
move the generated temp's owned reference into `target` instead of loading an
owned reference and then deleting the temp. For stack-mirrored cleanup roots,
the stack slot remains the owner: codegen stores the source value into the
target slot, clears the source slot without DECREFing it, and DECREFs only the
previous target slot value when needed.

The optimization is deliberately prefix-limited to generated temps. It must not
rewrite arbitrary user code such as:

```python
x = y
del y
```

Current source check:

```rust
name.starts_with("_dp_tmp_") || name.starts_with("_dp_typed_inline_")
```

That prefix check is only a guard for this specific adjacent-transfer peephole.
The peephole also requires local-location equality between the loaded source
and deleted source, different source/target locations, compatible LocalEnv
ownership, and a target storage shape that can receive the transferred owner.

## `_dp_typed_inline_*`

Produced by typed direct-call inlining. These names are allocated as typed stack
temps for inlined callables, arguments, results, and remapped callee locals,
for example:

```text
_dp_typed_inline_<runtime_module_id>_<local_function_id>_<temp_id>
_dp_typed_inline_arg_<runtime_module_id>_<local_function_id>_<temp_id>
_dp_typed_inline_result_<runtime_module_id>_<local_function_id>_<temp_id>
```

They are not Python source names. They represent inlined callee storage in the
caller and can participate in the same adjacent generated-temp ownership
transfer as `_dp_tmp_*` when the emitted typed IR has the exact store/delete
shape described above.

## `_dp_assign_value_*`

Produced by assignment lowering for RHS values that need source-order
presequencing before the assignment target is evaluated or mutated. These temps
remain real cleanup-visible locals when the later target operation can raise.

They are intentionally not part of the current generated-temp ownership
transfer. A benchmarked prototype let specialized indexed-field `SetAttr`
consume `_dp_assign_value_*` temps with a stealing store helper, but it grew
production JIT code size and regressed pystone apply throughput despite reducing
refcount counters. See `doc/todo/ownership.md` for the evidence and follow-up
direction.
