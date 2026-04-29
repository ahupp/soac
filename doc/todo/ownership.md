# Borrowed local ownership plan

Goal: make local/root slots the owning storage, and make SSA values loaded from
those slots borrowed by default whenever the consumer does not retain the value.
The first win should be fewer local-load INCREF/DECREF pairs in branch tests,
boolean expressions, and short-lived expression temporaries.

## Current shape

Typed demand planning already handles many expression inputs. For example,
`return x + y` plans both local loads as `BorrowedLocal` because `BinOp` child
demands are marked `borrowed_ok`.

The main visible gap is `Truthy`. Branch terms set the `Truthy` expression
itself to `I32Bool01`, but `annotate_typed_child_demands` does not propagate a
borrowed PyObject demand through `InstrTyped::Truthy`. Codegen then evaluates
the operand with `borrowed=false` and calls the truthiness helper as if it owned
the input.

There is a second, related gap in Python expression lowering: boolean and
conditional expressions use synthetic locals such as `_dp_target_1` and
`_dp_tmp_1`. Those stores force owned values even when the temporary is only an
SSA join for a later truthiness test or a return boundary.

## Target invariant

- Frame/root slots contain `null`, `immortal`, or owned references only.
- Borrowed values exist as SSA values, not as slot contents.
- A borrowed SSA value records its support root, initially a local/root slot.
- A borrowed SSA value may be used by non-retaining consumers: truthiness,
  comparisons, guards, arithmetic inputs, ordinary borrowed call arguments, and
  indexed reads.
- A borrowed SSA value is promoted with an explicit INCREF only when it crosses
  an ownership boundary: return, raise, store into a slot/container/global/cell,
  or a helper that steals or retains a reference.
- Phase 1 should avoid borrowed block params. If a value crosses a block edge,
  either reload it from the supporting slot in the successor or promote once at
  the boundary. A later phase can add validated borrowed phi/block-param
  support.

## Minimal cases to keep

Each case below currently either has no ownership plan for a local load under
`Truthy`, or forces an owned local load through a synthetic temp. After the
borrowed-local work, these should show `BorrowedLocal` for local inputs that are
consumed by non-retaining operations.

### 1. Branch on a local

```python
def branch_local(x):
    if x:
        return 1
    return 0
```

Current `InstrTyped` excerpt:

```text
function branch_local(x):
  bb0():
    if Truthy(x):
      then jump bb1
      else jump bb2

; [0] bb0 #0 Truthy result=Bool demand=I32Bool01 planned=I32Bool01
; [1] bb0 #0 Load result=PyObj(unknown)
```

Expected: the load of `x` is planned as borrowed from the local/root slot, and
truthiness consumes it without owning it. This is the smallest repro for the
missing `Truthy` child demand.

### 2. Comparison in a branch

```python
def cmp_branch(x, y):
    if x < y:
        return 1
    return 0
```

Current `InstrTyped` excerpt:

```text
function cmp_branch(x, y):
  bb0():
    if Truthy(BinOp(Lt, x, y)):
      then jump bb1
      else jump bb2

; [0] bb0 #0 Truthy result=Bool demand=I32Bool01 planned=I32Bool01 exact_int_branch
; [1] bb0 #0 BinOp result=PyObj(unknown)
; [2] bb0 #1 Load result=PyObj(unknown)
; [3] bb0 #2 Load result=PyObj(unknown)
```

Expected: `Truthy` propagates a borrowed demand into the comparison, and the
loads of `x` and `y` become `BorrowedLocal`. For exact-int branches, the
comparison should ideally feed the branch condition directly instead of
materializing a PyObject result only to test it.

### 3. Indexed local inputs in a branch

```python
def item_branch(seq, i):
    if seq[i]:
        return 1
    return 0
```

Current `InstrTyped` excerpt:

```text
function item_branch(seq, i):
  bb0():
    if Truthy(GetItem(seq, i)):
      then jump bb1
      else jump bb2

; [0] bb0 #0 Truthy result=Bool demand=I32Bool01 planned=I32Bool01
; [1] bb0 #0 GetItem result=PyObj(unknown) exact_list_item
; [2] bb0 #1 Load result=PyObj(unknown)
; [3] bb0 #2 Load result=PyObj(unknown)
```

Expected: local inputs `seq` and `i` are borrowed. A later extension can also
borrow the exact-list item result from the list support root when the item is
only consumed by truthiness and cannot escape.

### 4. Boolean value expression

```python
def and_value(x, y):
    return x and y
```

Current `InstrTyped` excerpt:

```text
function and_value(x, y):
  bb0():
    Store(_dp_target_1, x)
    if Truthy(_dp_target_1):
      then jump bb1
      else jump bb2

  bb1():
    Store(_dp_target_1, y)
    return _dp_target_1

  bb2():
    return _dp_target_1

; [1] bb0 #1 Load result=PyObj(unknown)
;     demand=PyObject { borrowed_ok: false } planned=PyObject { ownership: Owned }
; [3] bb0 #2 Load result=PyObj(unknown)
; [5] bb1 #4 Load result=PyObj(unknown)
;     demand=PyObject { borrowed_ok: false } planned=PyObject { ownership: Owned }
; [6] bb1 #5 Load result=PyObj(unknown)
;     demand=PyObject { borrowed_ok: false } planned=PyObject { ownership: Owned }
; [7] bb2 #6 Load result=PyObj(unknown)
;     demand=PyObject { borrowed_ok: false } planned=PyObject { ownership: Owned }
```

Expected: the initial truthiness test should borrow `x`. The selected result
still needs an owned reference at return, but ownership should be acquired once
at the return boundary, not through a synthetic temp slot store plus later
owned reloads.

### 5. Conditional expression

```python
def select_local(flag, x, y):
    return x if flag else y
```

Current `InstrTyped` excerpt:

```text
function select_local(flag, x, y):
  bb0():
    if Truthy(flag):
      then jump bb1
      else jump bb2

  bb1():
    Store(_dp_tmp_1, x)
    return _dp_tmp_1

  bb2():
    Store(_dp_tmp_1, y)
    return _dp_tmp_1

; [1] bb0 #0 Load result=PyObj(unknown)
; [3] bb1 #2 Load result=PyObj(unknown)
;     demand=PyObject { borrowed_ok: false } planned=PyObject { ownership: Owned }
; [4] bb1 #3 Load result=PyObj(unknown)
;     demand=PyObject { borrowed_ok: false } planned=PyObject { ownership: Owned }
; [6] bb2 #5 Load result=PyObj(unknown)
;     demand=PyObject { borrowed_ok: false } planned=PyObject { ownership: Owned }
; [7] bb2 #6 Load result=PyObj(unknown)
;     demand=PyObject { borrowed_ok: false } planned=PyObject { ownership: Owned }
```

Expected: `flag` is borrowed for truthiness. The selected `x`/`y` arm should be
borrowed until the return promotion point. This is a cross-block join case, so
it is a phase-2 test unless the first implementation chooses to reload/promote
inside each arm.

## Measurement plan

1. Add structured ownership tests that lower each snippet to `InstrTyped` and
   assert on typed metadata, not rendered strings. The first acceptance signal is
   the count and locations of `TypedPyObjectOwnershipPlan::BorrowedLocal`.

2. For the current gap, add a focused test that proves `Truthy(local)` annotates
   its local child with `TypedResultDemand::PYOBJECT_BORROWED_OK`.

3. For codegen, use CLIF/VCode inspection only as diagnostics. The production
   assertion should be structural: local loads consumed by truthiness should not
   request an owned PyObject result.

4. Add microbenchmarks that run tight loops over the cases above and collect:
   `runtime_incref`, `runtime_decref`, generated code size, and specialized
   apply throughput. The branch/comparison/item cases should reduce refcount
   traffic directly. The boolean/conditional cases should reduce traffic once
   synthetic temp ownership is replaced by borrowed SSA plus promotion.

5. Run the pystone benchmark after the focused tests pass. The expected broad
   signal is lower refcount counter volume in Proc0-style branch-heavy code,
   not only lower total CLIF size.

## Implementation steps

1. Done: teach typed demand propagation about `InstrTyped::Truthy`:

   ```rust
   InstrTyped::Truthy(op) => annotate_pyobject_borrowed_input_demand(op.value.as_mut())
   ```

   This should make the first three cases borrow local inputs without changing
   slot ownership semantics.

2. Done: make truthiness codegen consume a borrowed PyObject input when the operand's
   planned result is `BorrowedLocal` or `Immortal`. The truthiness helper now
   releases only owned inputs, so borrowed local and immortal operands do not
   get DECREF scaffolding.

3. Done for return terms: split "producer ownership" from the return ownership
   boundary. A returned local load can produce `BorrowedLocal`; codegen promotes
   borrowed values exactly at the return boundary before local cleanup releases
   the frame-owned slot. Stores remain conservative for now.

4. In progress: replace synthetic boolean/conditional temp ownership with
   explicit join handling. Direct-return conditional expressions now lower to
   arm-local return blocks, so `return x if flag else y` no longer materializes
   `_dp_tmp_*`. Boolean value expressions still need a representation that
   preserves the selected operand object across the truthiness branch without
   reloading from a possibly mutated support slot. Later: represent a
   borrowed-supported block value and validate the support root on all
   predecessors.

5. Extend `TypedPyObjectOwnershipPlan::BorrowedLocal` to carry support-root
   provenance before allowing borrowed values to cross block edges. The planner
   should validate that the supporting slot is not overwritten or deleted before
   the final use.

6. Keep stores conservative. Real local assignment still writes an owned slot.
   The optimization is to avoid taking ownership for reads that are only
   consumed by non-retaining operations, and to avoid synthetic temp stores that
   are only standing in for SSA joins.
