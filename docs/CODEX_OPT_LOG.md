# Codex Optimization Log

Chronological log of finalized performance changes and not-landed
optimization attempts made by Codex agents. Keep entries succinct: what
changed or was tried, which jj change id carried it when landed, the
benchmarked throughput delta, and the headline pre/post numbers.

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
