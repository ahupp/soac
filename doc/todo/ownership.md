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
- Phase 1 should avoid treating arbitrary block params as borrowed support
  roots. If a selected value crosses a block edge, it may use an explicit value
  block param, but the source must either be a validated local/root value or an
  owned carrier materialized on that predecessor. A later phase can add
  validated borrowed phi/block-param support for broader cases.

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

3. Done for return and raise terms: split "producer ownership" from the
   terminal ownership boundary. A returned or raised local load can produce
   `BorrowedLocal`; codegen promotes borrowed values exactly at the return/raise
   boundary before local cleanup releases the frame-owned slot. Stores remain
   conservative for now.

4. Done for local-selected boolean and conditional expressions: replace
   synthetic boolean/conditional temp ownership with
   explicit join handling. Direct-return and direct-raise conditional
   expressions now lower to arm-local terminal blocks, so `return x if flag else
   y` and `raise x if flag else y` no longer materialize `_dp_tmp_*`. Simple
   name assignments from conditional expressions expand to arm-local stores, so
   `z = x if flag else y` stores directly into `z` instead of selecting through
   `_dp_tmp_*`. Branch-test boolean and conditional expressions now lower
   directly to control flow, so `if x and y` and `if x if flag else y` no longer
   materialize `_dp_target_*` or `_dp_tmp_*` when only truthiness is needed.
   Effect-only conditional, boolean, and compare-chain expression statements now
   lower directly to control flow as well, so discarded selected values are not
   stored only to be DECREFed. Unary `not` over those branchable expressions
   also uses direct truthiness lowering instead of forcing the operand through a
   selected-value temp.
   Value-producing boolean expressions now use direct terminal/store lowering
   when every non-final selected operand is a forwardable local. `return x or g`
   and `raise x or y` return/raise the selected local directly and evaluate the
   final operand only in the final terminal block. `z = x or g` stores selected
   locals directly into `z` and stores the final operand directly into `z`.
   Nested boolean value consumers such as `sink(x or g)` use a `Value` block
   param when every non-final selected operand is a forwardable local: selected
   local predecessors forward the local, while a non-forwardable final operand
   materializes only a final-path `_dp_value_*` carrier.

   Remaining conservative cases: if a non-final selected operand is not a
   forwardable local (for example a global, attribute, call result, or cell
   load), lowering still uses the old owned selected-value temp. There is no
   validated support root for borrowing that selected object across the
   truthiness branch. Nested conditional-expression value consumers such as
   `sink(x if flag else y)` also keep the older `_dp_tmp_*` lowering for now; a
   direct `Value` block-param prototype was correctness-safe after requiring a
   forwardable arm, but it had no pystone code-size win and regressed the
   refcount-enabled apply benchmark in repeated runs. Same-local assignments
   such as `x = x or g` still emit
   `Store(x, x)` on the selected path to preserve the originally selected object
   if truthiness can observably mutate the frame local. A codegen same-root
   store no-op was prototyped, but the pystone refcount-enabled apply benchmark
   regressed despite a small code-size win, so that conversion is not kept in
   the benchmark-neutral borrowed-local base.

5. Done for local loads: `TypedPyObjectOwnershipPlan::BorrowedLocal` carries
   the supporting `LocalLocation`, and codegen only honors the borrowed plan
   when the planned location matches the actual local load. Before allowing
   borrowed values to cross block edges, the planner still needs to validate
   that the supporting slot is not overwritten or deleted before the final use.

6. Keep stores conservative. Real local assignment still writes an owned slot.
   The optimization is to avoid taking ownership for reads that are only
   consumed by non-retaining operations, and to avoid synthetic temp stores that
   are only standing in for SSA joins.

7. Done for typed module-constant fact recovery: after typed rewrites and
   inlining, `refresh_typed_function_value_facts` now reconstructs
   `ModuleConstant` provenance and immortal-refcount facts for typed constant
   loads instead of preserving a stale `unknown` PyObject fact. In pystone this
   moved 20 load sites from owned/unknown to immortal and reduced the
   borrowed-ok nonlocal owned-load bucket from 22 to 6. The runtime
   `runtime_incref` / `runtime_decref` counters did not change because module
   constants are loaded directly, but the typed ownership metadata no longer
   reports false owned requirements for those values.

8. Audited but not kept: a `BorrowedGlobalsDict` plan for `globals()` results
   passed directly as call arguments correctly converted four pystone
   class/module-initialization call results to borrowed current-globals values,
   but it produced no pystone code-size or block-count change and the
   refcount-enabled benchmark signal was noise-negative. Those sites are
   startup-only and not part of the dominant Proc0 refcount volume, so the extra
   ownership variant is not part of the current base.

9. Remaining pystone owned values are not currently safe borrowed conversions:
   borrowed-ok indexed global loads still need an owned result unless a
   surrounding v3 mechanical region proves a non-retaining hot path and owns the
   fallback/deopt shape; generic indexed-global loads do not have a stable
   support root because the module dict can be mutated before a reentrant
   consumer finishes. Borrowed-ok `GetItem`, `GetAttr`, `BinOp`, `Tuple`, and
   call results are produced values, not local-slot aliases. Exact-list item
   loads could become borrowed only with an item-type/no-reentrant proof or a
   split hot/fallback region like the existing v3 mechanical borrowed indexed
   global path.

10. Done for simple assignment target components and no-raise parameter RHS
    values: simple local target receiver/index operands flow directly into
    `SetAttr`/`SetItem` as borrowed local loads, removing `_dp_assign_obj_*` and
    `_dp_assign_index_*` owned slots for shapes such as `self.x = value` and
    `obj[i] = value`. The RHS also flows directly when there is one
    attribute/subscript target, the RHS and receiver are non-deleted function
    parameters, and a subscript index is a simple local load. This keeps
    CPython-visible order intact for the optimized case because the moved RHS
    and receiver loads cannot raise or have side effects, and the local index
    load cannot introduce side effects before the target update.

    Pystone `Record.__init__` now renders as direct `SetAttr(self, ..., Param)`
    operations with no `_dp_assign_obj_*` or `_dp_assign_value_*` stores. Its
    pre-inline refcount helper calls dropped from 116 to 66 after receiver
    forwarding, and then to 21 after no-raise RHS forwarding. The no-raise RHS
    benchmark `work/bench/nqtkkqssorqv_b6a9e4f37880` reduced verify counters
    from `runtime_incref=5,139,016` / `runtime_decref=6,345,198` to
    `runtime_incref=5,038,016` / `runtime_decref=6,244,198` versus
    `work/bench/nqtkkqssorqv_2c875f38f98f`, and moved pystone total code size
    from 64,657 to 64,505 bytes.

    The local-index subscript extension hit pystone `Proc8` / inlined `Proc8`
    shapes such as `Array1Par[IntLoc] = IntParI2`, moving total code size from
    64,505 to 64,103 bytes in `work/bench/nqtkkqssorqv_46891c667b41`. Runtime
    refcount counters did not change in that add-on because the removed slot was
    not a counted dynamic refcount site, but the emitted cleanup/code-size shape
    improved.

    Remaining conservative case: general RHS temps stay. Removing them would
    require either a SetAttr/SetItem operation whose child evaluation order
    explicitly models assignment RHS-before-target semantics, or a broader
    definite-bound/no-raise proof for the RHS and target loads. A plain
    tree-shaped `SetAttr(obj, attr, rhs)` evaluates the receiver before
    replacement in current BlockPy child order, so blindly removing
    `_dp_assign_value_*` would change observable `UnboundLocalError`/side-effect
    ordering.

11. Done for for-loop generated next-value temps: loop lowering still stores
    `next(iter, sentinel)` into a cleanup-visible `_dp_tmp_*` slot before
    assigning the target and deleting the temp, but it no longer emits the
    redundant generated self-store `_dp_tmp_* = _dp_tmp_*` before assigning the
    real loop target. This removes an owned reload/re-store of a compiler temp
    while preserving the existing cleanup path if target assignment raises. In
    pystone `Proc0`, the rendered specialized typed form no longer has
    `Store(_dp_tmp_1_8_1, _dp_tmp_1_8_1)` or
    `Store(_dp_tmp_1_8_4, _dp_tmp_1_8_4)`, and the inlined `Proc8` loop loses
    the analogous typed-inline self-store.

12. Done for adjacent generated-temp transfer into another local: typed codegen
    now recognizes `Store(target, _dp_tmp_*); Del(_dp_tmp_*)` and transfers the
    generated temp's ownership into the target binding instead of emitting an
    owned load followed by deleting the temp. The conservative shape is limited
    to compiler-generated `_dp_tmp_*` / `_dp_typed_inline_*` source names, so it
    does not rewrite user-visible `x = y; del y`.

    This matters for pystone's loop targets. In `Proc0` blocks `bb17` and
    `bb24`, the old shape was `INCREF tmp; stack_store tmp -> i; DECREF old_i;
    DECREF tmp`; the new shape is just `stack_store tmp -> i; DECREF old_i`.
    The same transfer handles stack-mirrored locals by treating the stack slot
    as the owner and the local-env value as borrowed from that slot.

    The benchmark `work/bench/nqtkkqssorqv_5473a00dca0a` reduced verify
    counters from `runtime_incref=4,840,015` / `runtime_decref=6,046,197` in
    `work/bench/nqtkkqssorqv_8effde18c609` to
    `runtime_incref=4,642,014` / `runtime_decref=5,848,196`, another 198,001
    fewer INCREFs and DECREFs. The refcount-enabled apply median moved from
    621,877 to 632,619 loops/s. The production refcount-enabled apply JIT code
    process also shrank from 135,742 bytes / 8,899 machine blocks to 134,747
    bytes / 8,859 blocks; the benchmark summary's "latest process" code-size
    line is the no-refcount diagnostic run, which moved in the opposite
    direction and is not the production size.

13. Remaining pystone refcount volume after the generated-temp transfer is
    concentrated in real produced-value cleanup, not in missed borrowed local
    loads. The benchmark `work/bench/nqtkkqssorqv_5473a00dca0a` reports
    `runtime_incref=4,642,014` / `runtime_decref=5,848,196`; by dynamic
    function total the largest buckets are `Proc0` at 5,035,975 calls,
    `Proc1` at 2,323,000, `Proc3` at 1,313,000, `Record.copy` at 606,000, and
    `Record.__init__` / the constructor entry at 808,008 combined.

    The largest static code-size bucket is still cold cleanup scaffolding:
    pre-inline `Proc0` has 740 static `soac_runtime_decref` calls versus 62
    `soac_runtime_incref` calls. That is mostly code-size pressure rather than
    the measured steady-state counter volume.

    A benchmarked steady-state target was stack-style attribute stores. Typed
    pystone still has shapes like `Store(_dp_assign_value_30, GetAttr(...))`,
    `SetAttr(..., _dp_assign_value_30)`, `Del(_dp_assign_value_30)`. CPython's
    bytecode stack naturally lets `STORE_ATTR` consume the owned stack value;
    SOAC's indexed-field store helper instead `INCREF`s the replacement and the
    following generated-temp `Del` then `DECREF`s it.

    That direct stealing-store prototype was not kept. The first benchmark,
    `work/bench/nqtkkqssorqv_fb961b404c1b`, reduced verify counters to
    `runtime_incref=4,642,014` / `runtime_decref=5,545,196`, removing 303,000
    counted DECREFs concentrated in `Proc1`. However, production
    refcount-enabled code size grew from 134,747 bytes / 8,859 machine blocks
    to 135,605 bytes / 8,962 blocks, and the apply median dropped from 632,619
    to 626,165 loops/s. After a no-refcount diagnostic fix in
    `work/bench/nqtkkqssorqv_3955033317b5`, the production apply median was
    still only 619,783 loops/s with the same larger code size. That makes the
    local peephole a poor base even though the counter direction was right.

    The likely missing shape is not "steal more locally" but "make produced
    value lifetimes stack-like before codegen": assignment RHS temps should be
    explicit owned SSA stack values whose consumer can take ownership without
    introducing a separate helper call, fallback island, or local cleanup
    exception. Otherwise the win in counted DECREFs is offset by larger control
    flow and helper plumbing.

    Remaining borrowed-load work is less direct: indexed `GetAttr` fast hits
    still `INCREF` their field result even when the consumer is borrowed-ok, but
    the fallback path currently returns an owned result through the same merge.
    Making that borrowed on the hot path needs either a split hot/fallback
    ownership result or deopt-only misses for the borrowed-result mode.
