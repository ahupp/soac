# Codex Optimization Log

Chronological log of finalized performance changes and not-landed
optimization attempts made by Codex agents. Keep entries succinct: what
changed or was tried, which jj change id carried it when landed, the
benchmarked throughput delta, and the headline pre/post numbers.

## 2026-04-13 - Remove no-op recursive leave calls

- jj change id: `kyxysmzx`
- summary: Direct-call and vectorcall-trampoline codegen no longer emits
  `dp_jit_leave_recursive_call` after the direct callee returns or after
  argument binding fails. In this vendored CPython, `Py_LeaveRecursiveCall`
  wraps a no-op; the direct-call path keeps the `Py_EnterRecursiveCall`
  C-stack guard and removes only the unpaired no-op leave call overhead.
- throughput: `+2.50%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `408266`, `418252`, `416438 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `421926`, `430101`, `426855 loops/s`

## 2026-04-13 - Not landed: thread owner type into indexed-field helpers

- jj change id: not landed
- summary: Tried passing the already-guarded exact owner type into
  `soac_runtime_probe_field_indexed` / `soac_runtime_store_field_indexed`
  so the helpers could skip reloading and re-deriving object type
  information on successful field get/store paths. The helper ABI change
  slightly reduced generated code size but did not improve specialized
  pystone throughput, so it was reverted.
- throughput: `-0.14%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `408266`, `418252`, `416438 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `421436`, `409964`, `415850 loops/s`

## 2026-04-13 - Recover direct Record constructor specialization

- jj change id: `rmsmspso`
- summary: Owner-type registration now also scans SOAC indexed module
  globals by direct indexed-dict slot lookup, so classes stored outside
  the plain module dict can register their `__init__` owner types without
  invoking module `__getattr__`. The specialized `Record.copy`
  constructor call in pystone now emits the direct
  `dp_jit_pytype_generic_alloc` / constructor-init path instead of always
  routing through `_PyObject_MakeTpCall`.
- throughput: `+27.31%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 100k loops x1: `300817 loops/s`
- post-change benchmark:
  - specialized pass, 100k loops x1: `382976 loops/s`

## 2026-04-12 - Deduplicate identical local failure cleanup blocks

- jj change id: `spxzrtuu`
- summary: Local failure cleanup lowering now reuses pending cleanup blocks
  when the concrete cleanup value list, forwarded value list, and continuation
  block are identical. An earlier arity-only sharing attempt reduced code size
  more aggressively but corrupted async cancellation cleanup, so the landed
  form keeps sharing limited to identical SSA cleanup inputs.
- throughput: `+3.68%` specialized pystone median; total pystone code size
  `-6.97%`; total pystone machine blocks `-6.06%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `275808`, `280431`, `273441 loops/s`
  - verify pass: `156370 loops/s`
  - total code size: `370500` bytes
  - total machine blocks: `22652`
- post-change benchmark:
  - specialized pass, 1M loops x3: `285947`, `279119`, `286187 loops/s`
  - verify pass: `155688 loops/s`
  - total code size: `344678` bytes
  - total machine blocks: `21280`

## 2026-04-12 - Use fixed-arity fallback calls for small guarded calls

- jj change id: `xqqowksq`
- summary: Guarded direct-call, direct-method, and constructor miss
  blocks now use the existing fixed-arity positional helper for fallback
  calls with at most three positional args. This avoids per-callsite
  vectorcall stack-slot setup on those cold miss paths while leaving the
  unspecialized generic positional-call path unchanged.
- throughput: `-0.35%` specialized pystone median in the finalized
  run; total pystone code size `-0.43%`; total pystone machine blocks
  `-0.17%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `281216`, `273692`, `276780 loops/s`
  - verify pass: `158201 loops/s`
  - total code size: `372092` bytes
  - total machine blocks: `22691`
- post-change benchmark:
  - specialized pass, 1M loops x3: `275808`, `280431`, `273441 loops/s`
  - verify pass: `156370 loops/s`
  - total code size: `370500` bytes
  - total machine blocks: `22652`

## 2026-04-12 - Share direct cleanup final return block

- jj change id: `lqtorwoq`
- summary: Direct-function lowering now shares null-cleanup blocks for
  source blocks that do not carry an active exception parameter. Blocks
  that need handled-exception state popping keep per-block cleanup. The
  shared and per-exception cleanup blocks are marked cold, and the
  benchmark summarizer now reports machine basic-block counts alongside
  code-size bytes.
- throughput: `+0.92%` specialized pystone median; total pystone code
  size `-7.66%`; total pystone machine blocks `-8.66%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `272054`, `276700`, `271604 loops/s`
  - verify pass: `168293 loops/s`
  - total code size: `402947` bytes
  - total machine blocks: `24842`
- post-change benchmark:
  - specialized pass, 1M loops x3: `274562`, `273407`, `281089 loops/s`
  - verify pass: `168352 loops/s`
  - total code size: `372092` bytes
  - total machine blocks: `22691`

## 2026-04-12 - Route direct-call null failures through the current step-null path

- jj change id: `quymqrww`
- summary: Direct-call null failures no longer build a local block that
  reloads and re-sets the current exception before branching to the
  active step-null continuation. The change also adds apply-mode
  regression coverage for direct-call and constructor exception
  propagation.
- throughput: `+1.06%` specialized pystone median after rebasing onto
  `sypsopvxttmk`; verify improved `+4.05%` and total pystone code size
  shrank by `1072` bytes
- pre-change benchmark:
  - specialized pass, 1M loops x3: `276181`, `278949`, `272400 loops/s`
  - verify pass: `160199 loops/s`
  - total code size: `404019` bytes
- post-change benchmark:
  - specialized pass, 1M loops x3: `279120`, `279927`, `274180 loops/s`
  - verify pass: `166693 loops/s`
  - total code size: `402947` bytes

## 2026-04-12 - Elide explicit error save/restore around decref cleanup

- jj change id: `omymzyom`
- summary: Removed the explicit current-exception save/restore sequence around
  owned-temp decref cleanup and routed those cleanup sites through a shared
  helper instead, relying on the runtime decref path to preserve the active
  Python exception.
- throughput: `-2.72%` specialized pystone median relative to `lrktzrpv`,
  with essentially unchanged verify throughput and a substantially smaller
  pystone JIT image
- pre-change benchmark:
  - specialized pass, 1M loops x3: `281062`, `281854`, `274855 loops/s`
  - verify pass: `161242 loops/s`
  - total code size: `447630` bytes
- post-change benchmark:
  - specialized pass, 1M loops x3: `282617`, `273427`, `271740 loops/s`
  - verify pass: `161285 loops/s`
  - total code size: `405732` bytes

## 2026-04-12 - Remove apply-mode specialization counters

- jj change id: `olrnwpvz`
- summary: Apply mode no longer lowers specialization profiling counters,
  no longer emits `dp_jit_record_top_value_sample`, and no longer logs
  specialization-runtime counter rows just because `SOAC_WORK_DIR` or
  `SOAC_LOG` is set. Profile and verify still record the same
  specialization set; only steady-state apply overhead changed.
- throughput: `+22.74%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `184066`, `187351`, `182357 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `221766`, `225928`, `228347 loops/s`

## 2026-04-11 - Disable profiled cold-block hints by default

- jj change id: `lwnxlmrx`
- summary: Kept the `block_entry` profiling pipeline and the apply/verify
  cold-block replay path, but gated the replay behind
  `SOAC_ENABLE_PROFILED_COLD_BLOCKS=1` so normal runs keep recording the
  counters without changing code layout.
- throughput: default path now matches the pre-replay baseline; the prior
  threshold experiments for the opt-in replay path stayed neutral to
  slightly negative (`166579` baseline median, `166428` at 50%, `162351`
  at 80%), so the hint now ships opt-in only

## 2026-04-11 - Mark rarely visited profiled blocks cold

- jj change id: `oxrtzwlp`
- summary: Replayed `block_entry` counters during apply/verify JIT
  lowering and marked non-entry blocks visited at most 1% as often as
  the function entry block as Cranelift `cold` blocks. This is a layout
  hint only; the short pystone validation run showed no code-size
  counter change.
- throughput: `-0.38%` median specialized pystone in a short
  100-loop validation run; treated as noise-level / neutral
- pre-change benchmark:
  - specialized pass, 100 loops x3: `158967`, `157293`, `155422 loops/s`
  - machine code size total/max: `1018394` / `120176` bytes
- post-change benchmark:
  - specialized pass, 100 loops x3: `156690`, `158210`, `153109 loops/s`
  - machine code size total/max: `1018394` / `120176` bytes

## 2026-04-08 - Inline runtime guard and indexed-field helpers

- jj change id: `kkoolpkp`
- summary: Type/version guards now inline through soac-runtime, indexed
  field helpers use direct dict/inline-values access instead of
  `_PyObject_GetDictPtr`, and the opt-in unsound indexed field-store path
  reports hit/miss status instead of returning an owned temporary.
- throughput: `+4.01%` 100k default-specialized pystone; `+12.03%`
  100k opt-in unsound indexed-store pystone
- pre-change benchmark:
  - default specialized: `154514 loops/s`
  - opt-in unsound indexed stores: `142627 loops/s`
- post-change benchmark:
  - default specialized: `160710 loops/s`
  - opt-in unsound indexed stores: `159810 loops/s`
  - same-run stock CPython: about `555k loops/s`

## 2026-04-08 - Call PyLong slots directly for exact-int specialization

- jj change id: `tuyrzlpu`
- summary: Exact-`int` binary operator specialization now emits imports
  for the profiled `PyLong_Type` number slots and rich-compare slot
  instead of calling the generic Rust `dp_jit_exact_long_binary_op`
  dispatch helper. The runtime JIT symbol table binds those imports to
  CPython's `PyLong_Type.tp_as_number` / `tp_richcompare` function
  pointers at registration time.
- throughput: `+3.39%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `154501`, `155750`, `141058 loops/s`
  - stock CPython: `541272 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `158845`, `159744`, `160164 loops/s`
  - stock CPython: `550585 loops/s`

## 2026-04-08 - Reuse the direct-call entry pointer load

- jj change id: `tuyrzlpu`
- summary: Direct-call codegen now carries `FunctionEnv.direct_code_ptr`
  out of the metadata / lazy-compile check and reuses it for
  `call_indirect`, removing the duplicate direct-code-pointer load and
  null check from the fast path.
- throughput: `+1.48%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `158845`, `159744`, `160164 loops/s`
  - stock CPython: `550585 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `156111`, `162114`, `163148 loops/s`
  - stock CPython: `551829 loops/s`

## 2026-04-08 - Inline next-or-sentinel iterator progress

- jj change id: `tuyrzlpu`
- summary: Codegen now recognizes transformed calls to
  `__soac__.next_or_sentinel(iterator)` and emits a native iterator
  helper call instead of vectorcalling the transformed Python runtime
  helper. The helper calls `PyIter_NextItem`, returns the module's
  `ITER_COMPLETE` singleton when the iterator is exhausted, and leaves
  real iterator errors on the existing null-return exception path.
- throughput: `+8.06%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `156111`, `162114`, `163148 loops/s`
  - stock CPython: `551829 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `175180`, `159439`, `178348 loops/s`
  - stock CPython: `568659 loops/s`

## 2026-04-08 - Avoid KeyError allocation in global-load fallback

- jj change id: `lzqutouv`
- summary: The JIT runtime global-load fallback now probes exact dict
  globals and dict builtins with `PyDict_GetItemRef`, preserving the
  owned-reference contract without first calling mapping subscript,
  constructing `KeyError`, clearing it, and then looking in builtins.
- throughput: `+15.81%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `214770`, `217031`, `223132 loops/s`
  - stock CPython: `559279 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `252020`, `251340`, `248906 loops/s`
  - stock CPython: `552461 loops/s`

## 2026-04-08 - Fast-path exact list item helpers

- jj change id: `spsxlton`
- summary: The existing JIT getitem/setitem helpers now handle exact
  `list` with exact compact-`int` index directly: decode compact long
  indexes in Rust, normalize in-range negative indices, use direct
  `PyList_GET_ITEM` / `PyList_SET_ITEM` access, and fall back to the
  generic item protocol for mismatched, big-int, or out-of-range cases.
- throughput: `+3.12%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `308879`, `313589`, `309820 loops/s`
  - stock CPython: `551629 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `319499`, `320812`, `308803 loops/s`
  - stock CPython: `558357 loops/s`

## 2026-04-08 - Not landed: post-list-fastpath micro-optimizations

- jj change id: not landed
- summary: Tried several follow-up pystone optimizations after the
  exact-list helper change and reverted each candidate because it was
  benchmark-negative or too close to noise.
- attempts:
  - generated exact-list / compact-int getitem fast path in CLIF:
    median `319499 -> 314717 loops/s`, `-1.50%`
  - direct `ob_type` checks inside the exact-list helper: median
    `319499 -> 320632 loops/s`, `+0.35%`; treated as noise and not
    landed
  - generated singleton-truth fast path before `dp_jit_is_true`:
    median `319499 -> 307591 loops/s`, `-3.73%`
  - singleton fast path inside `dp_jit_is_true`: median
    `319499 -> 304941 loops/s`, `-4.56%`
  - branch-context richcompare-truth helper: median
    `319499 -> 306582 loops/s`, `-4.04%`

## 2026-04-09 - Profile conditional branch locality

- jj change id: `wvotvvly`
- summary: Profile/apply mode now records each conditional terminator's
  post-truthiness boolean as a `branch_outcomes` top-value counter, replays
  false-vs-true counts from `profile.bin`, and inverts false-hot specialized
  JIT branches so the hotter edge is the Cranelift true / first edge.
- throughput: `+0.15%` median versus the first parent run; repeat
  parent run was lower, so treat the measured change as benchmark noise
- pre-change benchmark:
  - specialized pass, 1M loops x3: `310384`, `315461`, `313523 loops/s`
  - repeat specialized pass, 1M loops x3: `299188`, `313244`,
    `308169 loops/s`
  - stock CPython: `550680 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `313980`, `319739`, `310154 loops/s`
  - stock CPython: `552602 loops/s`

## 2026-04-09 - Not landed: Cranelift Fast calling-convention experiments

- jj change id: not landed
- summary: Tried Cranelift `CallConv::Fast` on SOAC-internal ABIs. The
  direct-body variant changed the compiled transformed-Python body and
  matching indirect-call signatures. The runtime-helper variant changed only
  local `soac_runtime_*` CLIF helper definitions and matching local imports.
  Neither produced a benchmark-visible pystone win.
- attempts:
  - direct transformed-Python body ABI: median `313500 -> 308959 loops/s`,
    `-1.45%`
  - runtime CLIF helper ABI: median `313500 -> 312062 loops/s`, `-0.46%`
- baseline benchmark:
  - specialized pass, 1M loops x3: `313500`, `297235`, `317124 loops/s`
  - stock CPython: `544436 loops/s`
- direct-body Fast benchmark:
  - specialized pass, 1M loops x3: `308959`, `318225`, `291427 loops/s`
  - stock CPython: `523614 loops/s`
- runtime-helper Fast benchmark:
  - specialized pass, 1M loops x3: `316166`, `312062`, `309037 loops/s`
  - stock CPython: `550054 loops/s`

## 2026-04-09 - Apply-mode raw indexed stores

- jj change id: `qrutwqnr`
- summary: Apply mode now emits raw indexed stores for specialized
  module-global and split instance-field writes, bypassing CPython
  dict/object/type observer and insertion-order maintenance on guarded hits.
- throughput: `+10.29%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `250439`, `249170`, `247683 loops/s`
  - perf-context run: `247131 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `274809`, `279150`, `253522 loops/s`
  - perf-context run: `255713 loops/s`

## 2026-04-10 - Upgrade Cranelift to 0.130.1

- jj change id: `vyqwvlks`
- summary: Upgraded the Cranelift dependency family from `0.125` to
  `0.130.1`, aligned the direct `gimli` dependency with Cranelift's
  unwind types, and kept the regenerated snapshot formatting changes.
- throughput: `+1.94%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `273439`, `267436`, `275214 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `278749`, `281497`, `276426 loops/s`

## 2026-04-10 - Use process-JIT direct calls for SOAC function targets

- jj change id: `wwosynst`
- summary: Process-JIT batches now predeclare reachable SOAC functions and
  emit CLIF direct `call`s for supported direct edges. Unsupported edges use
  the generic Python call fallback, and warmed direct-context lookups avoid
  cloning the lowered `BlockPyFunction` after compilation.
- throughput: `+179.55%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `89790`, `86473`, `86037 loops/s`
  - perf-context run: `83929 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `247158`, `240873`, `241731 loops/s`
  - perf-context run: `227895 loops/s`

## 2026-04-10 - Remove direct-entry tracing from generated code

- jj change id: `oxqvnxtl`
- summary: Removed the generated direct-entry trace helper import and calls,
  the runtime symbol binding, and the helper that checked the process
  environment on every direct JIT entry.
- throughput: `+25.43%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `90588`, `89972`, `92372 loops/s`
  - perf-context run: `88371 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `114328`, `107795`, `113621 loops/s`
  - perf-context run: `95269 loops/s`

## 2026-04-10 - Enable direct field-index specialization

- jj change id: `vytxokyr`
- summary: Same-module direct-function compilation now receives module globals,
  so apply/verify mode can resolve profiled split-dict owner layouts and emit
  field-indexed instance load/store fast paths. Also removed the leftover
  `SOAC_BIND_TRACE` argument-binding debug path.
- throughput: `+6.20%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `85166`, `114419`, `115642 loops/s`
  - perf-context run: `107731 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `115465`, `123032`, `121511 loops/s`
  - perf-context run: `117328 loops/s`

## 2026-04-10 - Pass thread state to indexed field stores

- jj change id: `xuwyyrwr`
- summary: The indexed field-store runtime helper now receives the generated
  function's existing `PyThreadState` pointer and uses it when decrefing a
  replaced field value, avoiding one helper-local TLS lookup on successful raw
  field stores.
- throughput: `+3.58%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `115465`, `123032`, `121511 loops/s`
  - perf-context run: `117328 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `131076`, `121536`, `125865 loops/s`
  - perf-context run: `116351 loops/s`

## 2026-04-10 - Not landed: split exact-int truth helper

- jj change id: not landed (`voqqtors`)
- summary: Tried routing exact-`int` `not` / internal truth unary
  specialization through a new helper returning raw `nb_bool` as `i32`, then
  materializing `Py_True` / `Py_False` through the typed bool path. The
  specialization set did not change, and the removed object-returning exact-long
  unary helper path was too small in pystone to justify the extra split.
- throughput: `-1.01%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `126977`, `126471`, `123701 loops/s`
  - perf-context run: `125578 loops/s`
- attempted benchmark:
  - specialized pass, 1M loops x3: `124219`, `127264`, `125195 loops/s`
  - perf-context run: `120165 loops/s`

## 2026-04-11 - Thread tstate through hot JIT helpers

- jj change id: `sosmzxqw`
- summary: Threaded the existing `PyThreadState` parameter through the hot JIT
  helper paths, including keyword and unpacked-call helper lowering, so those
  paths stop doing helper-local thread-state/TLS lookups. The specialized and
  verify specialization sets stayed identical across the before/after runs.
- throughput: `+69.50%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `101666`, `107664`, `103292 loops/s`
  - perf-context run: `100400 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `175063`, `176168`, `175076 loops/s`
  - perf-context run: `172766 loops/s`

## 2026-04-12 - Honor effect-only demand for LocalEnv stores

- jj change id: `xykryxmq`
- summary: Statement-position LocalEnv store/delete producers now return
  `NoValue` when the result is not consumed, avoiding owned-`None`
  materialization for those producer paths. Against the verify-refcount-counter
  base, specialization sets and verify hit/fallback counters stayed unchanged.
- throughput: `+9.32%` specialized pystone median; code size `-2.90%`;
  applied refcount ops unchanged
- pre-change benchmark:
  - specialized pass, 1M loops x3: `170756`, `172461`, `166913 loops/s`
  - pystone JIT code bytes: `933661`
  - pystone verify refcount ops: `20956626`
- post-change benchmark:
  - specialized pass, 1M loops x3: `185930`, `189987`, `186673 loops/s`
  - pystone JIT code bytes: `906622`
  - pystone verify refcount ops: `20956626`

## 2026-04-12 - Reduce JIT LocalEnv stack mirrors

- jj change id: `mnplvqtw`
- summary: Direct-entry params and cleanup-only locals now travel through
  planned block params where possible, and the JIT allocates physical stack
  slots only for remaining stack-backed paths. This removes entry
  store/load roundtrips and avoids preserving stack mirrors only for
  representation compatibility.
- throughput: `+26.62%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `221766`, `225928`, `228347 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `280740`, `287558`, `286068 loops/s`

## 2026-04-12 - Share local failure cleanup blocks by logical locals

- jj change id: `onzmnylr`
- summary: Cleanup-null failure paths now share cleanup blocks when they release
  the same ordered logical locals, while passing the current SSA values as
  block params. Generator/coroutine closure bindings use the same logical-local
  cleanup key as ordinary closure bindings.
- throughput: `-1.65%` specialized pystone median; code size `-9.79%`;
  machine blocks `-7.96%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `284414`, `291437`, `278717 loops/s`
  - pystone JIT code bytes: `344486`
  - pystone machine blocks: `21281`
- post-change benchmark:
  - specialized pass, 1M loops x3: `279717`, `277360`, `281715 loops/s`
  - pystone JIT code bytes: `310747`
  - pystone machine blocks: `19587`

## 2026-04-13 - Remove redundant refcount null guards

- jj change id: `rxmtwtlm`
- summary: JIT refcount release/acquire sites now call the runtime refcount
  helper directly and rely on the helper's own null/immortal checks, avoiding a
  duplicate caller-side branch pair that survived runtime helper inlining.
- throughput: `+5.55%` specialized pystone median; code size `-13.78%`;
  machine blocks `-12.60%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `290599`, `284774`, `285709 loops/s`
  - pystone JIT code bytes: `310747`
  - pystone machine blocks: `19587`
- post-change benchmark:
  - specialized pass, 1M loops x3: `303904`, `297016`, `301574 loops/s`
  - pystone JIT code bytes: `267926`
  - pystone machine blocks: `17119`

## 2026-04-13 - Hoist JIT runtime constant loads

- jj change id: `uunnzzqx`
- summary: Runtime constants loaded from the module constant table now happen
  once in the direct-entry setup and are reused across generated blocks,
  instead of reloading `None`, bools, deleted-sentinel, and empty-tuple for
  every block.
- throughput: `-0.03%` specialized pystone median; code size `-2.35%`;
  machine blocks `-0.71%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `306741`, `306534`, `308409 loops/s`
  - pystone JIT code bytes: `267926`
  - pystone machine blocks: `17119`
- post-change benchmark:
  - specialized pass, 1M loops x3: `302549`, `307871`, `306656 loops/s`
  - pystone JIT code bytes: `261630`
  - pystone machine blocks: `16998`

## 2026-04-13 - Optimize vectorcall argument binding

- jj change id: `rvqxpmyw`
- summary: Direct vectorcall argument binding now uses a precomputed function
  binding plan and writes owned argument values directly into the trampoline's
  output buffer, avoiding per-call `bound_args`, `assigned`, and positional
  parameter-index vector allocations.
- throughput: `+12.54%` specialized pystone median; code size unchanged;
  machine blocks unchanged
- pre-change benchmark:
  - specialized pass, 1M loops x3: `305154`, `311978`, `307370 loops/s`
  - pystone JIT code bytes: `261630`
  - pystone machine blocks: `16998`
- post-change benchmark:
  - specialized pass, 1M loops x3: `336900`, `349622`, `345921 loops/s`
  - pystone JIT code bytes: `261630`
  - pystone machine blocks: `16998`

## 2026-04-13 - Call imported JIT helpers directly

- jj change id: `zwsqzyuv`
- summary: Imported helper calls now keep the Cranelift `Linkage::Import`
  declaration directly instead of defining a local helper-pointer data slot and
  trampoline function per imported symbol. This removes the extra trampoline
  call/return hop, but leaves Cranelift's far external target materialization at
  each call site.
- throughput: `+9.62%` specialized pystone median; code size `+1.37%`;
  machine blocks `+0.02%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `319541`, `347491`, `338516 loops/s`
  - pystone JIT code bytes: `174333`
  - pystone machine blocks: `11257`
- post-change benchmark:
  - specialized pass, 1M loops x3: `373208`, `366708`, `371067 loops/s`
  - pystone JIT code bytes: `176723`
  - pystone machine blocks: `11259`

## 2026-04-13 - Outline decref dealloc preservation

- jj change id: `pmmnxlnu`
- summary: The exception-preserving refcount-zero dealloc path now lives in a
  non-inlined generated runtime CLIF helper and runtime-CLIF calls to generated
  runtime functions are remapped as local JIT functions, removing duplicated
  dealloc-preservation code from inlined decref/store fast paths.
- throughput: `+0.22%` specialized pystone median; code size `-5.44%`;
  machine blocks `+0.03%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `411595`, `406691`, `405697 loops/s`
  - pystone JIT code bytes: `271819`
  - pystone machine blocks: `17385`
- post-change benchmark:
  - specialized pass, 1M loops x3: `417360`, `407568`, `405675 loops/s`
  - pystone JIT code bytes: `257025`
  - pystone machine blocks: `17390`
