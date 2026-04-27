# Overnight Performance Log

## 2026-04-27 - Inline non-null indexed field stores

- baseline: `work/bench/qkzxlxtsvpuq_addfd7675506`
  - specialized apply median: `566978 loops/s`
  - specialized apply mean: `568132 loops/s`
  - verify pass: `340618 loops/s`
  - no-refcount diagnostic median: `736336 loops/s`
  - latest summarized pystone code size: `65372 bytes`, `3862` machine blocks
- observation: `soac_runtime_store_field_indexed_inline_values_trusted` was a
  top standalone profile symbol at about `5.68%`. Specialized indexed field
  stores already know the owner type/version and field index, so the common
  already-populated split-slot update looked like a candidate for direct JIT
  emission.
- attempted change: inline the non-null split-value update in the typed indexed
  `SetAttr` path: verify no materialized dict, compute the inline values pointer,
  check validity/capacity, load the old slot value, `INCREF` the replacement,
  store it, and `DECREF` the old value. The existing trusted helper remains on a
  cold path for first insertion, and normal generic setattr fallback remains on
  layout miss.
- rejected result: `work/bench/tvtwsqpuolsr_fa0397cb8d6b`
  - specialized apply median: `566818 loops/s` (`-0.03%`)
  - specialized apply mean: `570183 loops/s` (`+0.36%`)
  - verify pass: `329895 loops/s` (`-3.15%`)
  - no-refcount diagnostic median: `743108 loops/s` (`+0.92%`)
  - latest summarized pystone code size: `67993 bytes`, `4025` machine blocks
  - code-size delta: `+2621 bytes`, `+163` machine blocks
- reason rejected: the production median was not materially positive, verify
  regressed, and the inlined layout/refcount path grew hot pystone functions
  substantially. The helper-call overhead does not justify duplicating the store
  shape at current callsites.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo test -p
  soac_jit field_indexed_setattr_guard_miss_keeps_fallback_when_operands_are_replay_safe
  -- --nocapture` passed; `cargo check -p soac_jit --tests` passed; `just
  benchmark` produced the rejected result above. The experiment was then
  reverted.
- next baseline: `work/bench/qkzxlxtsvpuq_addfd7675506`

## 2026-04-27 - Guarded scalar index for generic getitem

- baseline: `work/bench/qkzxlxtsvpuq_addfd7675506`
  - specialized apply median: `566978 loops/s`
  - specialized apply mean: `568132 loops/s`
  - verify pass: `340618 loops/s`
  - no-refcount diagnostic median: `736336 loops/s`
  - latest summarized pystone code size: `65372 bytes`, `3862` machine blocks
- observation: the deep profile still showed generic `PyNumber_Add` cost, and
  pystone has subscript index expressions such as `i + 1` feeding generic
  getitem. The typed emitter already had a guarded I64 index path for exact-list
  plans, so a narrower generic getitem experiment could avoid materializing the
  index expression before the fallback.
- attempted change: when a typed generic getitem has no shape counter or
  exact-list plan, emit the object first, guard and scalarize an index expression
  that the existing typed index analysis accepts, box only that scalar index for
  `dp_jit_pyobject_getitem`, and keep the original generic index/getitem
  sequence on the cold fallback edge.
- rejected result: `work/bench/omlmmwmytllp_8b9a02b675d5`
  - specialized apply median: `547483 loops/s` (`-3.44%`)
  - specialized apply mean: `551342 loops/s` (`-2.96%`)
  - verify pass: `328559 loops/s` (`-3.54%`)
  - no-refcount diagnostic median: `745978 loops/s` (`+1.31%`)
  - latest summarized pystone code size: `65948 bytes`, `3893` machine blocks
  - code-size delta: `+576 bytes`, `+31` machine blocks
- reason rejected: the scalar hot path removes some no-refcount cost, but the
  production path pays for extra guards, fallback control flow, and boxed-key
  materialization before the generic getitem call. The result regresses apply
  and verify while increasing generated code size.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo test -p
  soac_jit typed_generic_getitem_with_scalarizable_index_emits_guarded_index_path
  -- --nocapture` passed; `cargo check -p soac_jit --tests` passed; `just
  benchmark` produced the rejected result above. The experiment was then
  reverted.
- next baseline: `work/bench/qkzxlxtsvpuq_addfd7675506`

## 2026-04-27 - Direct runtime primitive for range calls

- baseline: `work/bench/qkzxlxtsvpuq_addfd7675506`
  - specialized apply median: `566978 loops/s`
  - specialized apply mean: `568132 loops/s`
  - verify pass: `340618 loops/s`
  - no-refcount diagnostic median: `736336 loops/s`
  - latest summarized pystone code size: `65372 bytes`, `3862` machine blocks
- observation: the deep profile still showed `py_vectorcall_hook` and range
  construction in the hot path. The direct runtime primitive mechanism already
  handled builtins like `ord`, `chr`, `len`, and `iter`, so `range` looked like
  a plausible next fixed-arity builtin candidate.
- attempted change: add direct runtime primitive descriptors for `range(stop)`,
  `range(start, stop)`, and `range(start, stop, step)`, routing them through
  narrow runtime helpers that call `PyObject_Vectorcall` on `PyRange_Type`.
- rejected result: `work/bench/uxrpukyqykko_0e41e1b72d85`
  - specialized apply median: `548850 loops/s` (`-3.20%`)
  - specialized apply mean: `546829 loops/s` (`-3.75%`)
  - verify pass: `322062 loops/s`
  - no-refcount diagnostic median: `702990 loops/s` (`-4.53%`)
  - latest summarized pystone code size: `65307 bytes`, `3847` machine blocks
  - code-size delta: `-65 bytes`, `-15` machine blocks
- reason rejected: the helper made generated code slightly smaller, but it still
  performs generic `PyRange_Type` vectorcall allocation and does not address the
  real steady-state cost of range iteration, iterator state, `__next__`, or
  `StopIteration` handling. The production apply and verify regressions are too
  large to keep for the small size reduction.
- validation before rejection: `just fmt-rust soac_jit` passed; runtime crate
  formatting and `cargo check --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo test -p soac_jit
  direct_abi -- --nocapture` passed; `cargo check -p soac_jit --tests` passed;
  `just pytest-fast tests/test_runtime_builtin_primitives.py -q` passed;
  `just benchmark` produced the rejected result above. The experiment was then
  reverted.
- next baseline: `work/bench/qkzxlxtsvpuq_addfd7675506`

## 2026-04-27 - Inline guarded method direct-call stores

- baseline: `work/bench/qkzxlxtsvpuq_addfd7675506`
  - specialized apply median: `566978 loops/s`
  - specialized apply mean: `568132 loops/s`
  - verify pass: `340618 loops/s`
  - no-refcount diagnostic median: `736336 loops/s`
  - latest summarized pystone code size: `65372 bytes`, `3862` machine blocks
- observation: the deep profile still showed hot `Record.copy` execution even
  though method direct-call targets were already discovered. The typed inline
  rewriter only handled guarded callable calls, so method calls could direct-call
  but could not inline the target body into the caller.
- attempted change: allow same-module method direct-call plans to select an
  inline body, lower the typed inline guard as an exact receiver type/version
  check, bind the receiver temp as `self`, and leave a generic method call on
  the fallback edge.
- rejected result: `work/bench/nswnkxpqnpxn_73b1924dd064`
  - specialized apply median: `506102 loops/s` (`-10.74%`)
  - specialized apply mean: `504145 loops/s` (`-11.26%`)
  - verify pass: `311432 loops/s`
  - no-refcount diagnostic median: `615548 loops/s` (`-16.40%`)
  - latest summarized pystone code size: `65764 bytes`, `3876` machine blocks
  - code-size delta: `+392 bytes`, `+14` machine blocks
- reason rejected: the rewrite did remove some runtime direct-call/field/refcount
  counter traffic, but it grew `Proc1` and made the production apply path much
  slower. The current inline shape pays too much in receiver temps, guard/fallback
  control flow, and duplicated inlined body code for the `Record.copy` call.
- validation before rejection: Rust formatting passed; the focused `soac_opt`
  and `soac_ir_typed` tests passed; `cargo check -p soac_jit --tests` passed;
  `just benchmark` produced the rejected result above. The experiment was then
  reverted.
- next baseline: `work/bench/qkzxlxtsvpuq_addfd7675506`

## 2026-04-26 - Fast path compact-ASCII unicode getitem in helper

- baseline: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - specialized apply mean: `592714 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: `Func2` executes two `str[int]` getitems per hot call, but the
  current getitem shape planner only recognizes exact-list/exact-int, so exact
  compact-ASCII string indexing falls through to `PyObject_GetItem`.
- attempted change: add a narrow fast path inside `dp_jit_pyobject_getitem` for
  exact compact-ASCII unicode plus exact compact int. It normalizes negative
  indices, returns `PyUnicode_FromOrdinal` for in-bounds characters, and
  otherwise falls back to the existing generic helper call.
- rejected result: `work/bench/knlskolznnxw_5d96aeacb6e2`
  - specialized apply median: `583186 loops/s` (`-0.72%`)
  - specialized apply mean: `584273 loops/s` (`-1.42%`)
  - verify pass: `346255 loops/s`
  - no-refcount diagnostic median: `776087 loops/s` (`+0.73%`)
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
  - code-size delta: `+0 bytes`, `+0` machine blocks
- reason rejected: the helper path reduced some no-refcount and verify cost but
  still regressed production apply. A generic helper branch is too broad for
  this benchmark; a future unicode getitem attempt should be profile-planned at
  the typed op site instead of inserted into all generic getitems.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `just benchmark` produced the rejected result
  above. The experiment was then reverted.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Skip trusted field-store refcounts for identical value

- baseline: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - specialized apply mean: `592714 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: `soac_runtime_store_field_indexed_inline_values_trusted` remains
  the largest helper symbol in the profile. Like list slot replacement, storing
  the exact same pointer into an existing field has net-zero refcount effects.
- attempted change: in the trusted inline-values field-store runtime helper,
  return success immediately when `old_value == value`, skipping the replacement
  `INCREF`, split-slot store, and old-value `DECREF`.
- rejected result: `work/bench/knlskolznnxw_06b618a45800`
  - specialized apply median: `580585 loops/s` (`-1.16%`)
  - specialized apply mean: `582904 loops/s` (`-1.65%`)
  - verify pass: `347281 loops/s`
  - no-refcount diagnostic median: `772330 loops/s` (`+0.24%`)
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
  - code-size delta: `+0 bytes`, `+0` machine blocks
- reason rejected: this helped verify and the no-refcount diagnostic, but the
  production apply median and mean regressed. The extra branch in the hot helper
  is not worth keeping without a production-mode win.
- validation before rejection: `cargo fmt --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo check --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo check -p soac_jit --tests`
  passed; `just benchmark` produced the rejected result above. The experiment
  was then reverted.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Skip refcount work for identical exact-list replacement

- baseline: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - specialized apply mean: `592714 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: exact-list setitem specialization still emitted
  `INCREF replacement; store; DECREF old_item` at each hot store site. For an
  identical old and new item, those refcount operations are net zero and the
  store is redundant.
- attempted change: share a helper across the exact-list setitem emitters that
  checks `old_item == replacement`; on equality it skips the replacement
  `INCREF`, slot store, and old-item `DECREF`.
- rejected result: `work/bench/knlskolznnxw_23758f55b578`
  - specialized apply median: `589142 loops/s` (`+0.30%`)
  - specialized apply mean: `586624 loops/s` (`-1.03%`)
  - verify pass: `335085 loops/s`
  - no-refcount diagnostic median: `757259 loops/s` (`-1.71%`)
  - latest summarized pystone code size: `60408 bytes`, `3610` machine blocks
  - code-size delta: `+97 bytes`, `+1` machine block
- reason rejected: the median-only gain is too small to trust, while the mean,
  no-refcount diagnostic, and code size all moved the wrong way. The extra
  branch is not justified without a direct profile signal that equal old/new
  list items dominate these stores.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `just benchmark` produced the rejected result
  above. The experiment was then reverted.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Fact-gated exact-int rich-compare fast path

- baseline: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: the broad exact-int rich-compare fast path regressed by adding
  guard code to every comparison. A narrower variant looked plausible if
  existing typed facts already identified both operands as exact `int`.
- attempted change: route generic rich-compare BinOps through the compact-int
  guard/unbox/compare fast path only when `py_facts_for_arg` reports exact
  `int` facts for both operands; otherwise keep the current generic
  `PyObject_RichCompare` emission.
- rejected result: `work/bench/knlskolznnxw_58d1cb83fe40`
  - specialized apply median: `579212 loops/s` (`-1.39%`)
  - verify pass: `335247 loops/s`
  - no-refcount diagnostic median: `766465 loops/s` (`-0.52%`)
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
  - code-size delta: `+0 bytes`, `+0` machine blocks
- reason rejected: the identical code size and counter summary show that the
  fact-gated path did not attach to production pystone. The current typed facts
  are not precise enough at those generic comparison sites, so the benchmark
  result is just noise plus a lower median.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `just benchmark` produced the rejected result
  above. The experiment was then reverted.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Cranelift `speed` instead of `speed_and_size`

- baseline: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: the current benchmark uses Cranelift `speed_and_size`. Since
  pystone is dominated by hot generated functions, `speed` looked like a
  possible low-effort throughput win if the larger-code tradeoff was acceptable.
- attempted change: run the same current source with
  `SOAC_CRANELIFT_OPT_LEVEL=speed` and no source edits.
- rejected result: `work/bench/knlskolznnxw_c01f36dad6b4`
  - specialized apply median: `576818 loops/s` (`-1.80%`)
  - verify pass: `333791 loops/s`
  - no-refcount diagnostic median: `766859 loops/s` (`-0.47%`)
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- reason rejected: `speed` did not change the summarized pystone code size and
  regressed the production apply median. Keep `speed_and_size` for this
  benchmark path.
- validation before rejection: `SOAC_CRANELIFT_OPT_LEVEL=speed just benchmark`
  produced the rejected result above; no source change was kept.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Direct-call return facts to scope unary `not`

- baseline: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: requiring exact-int facts before the unary-`not` fast path lost
  the pystone win because profiled direct calls to `Func2`/`Func3` did not carry
  precise return facts at generic unary emission time.
- attempted change: infer simple exact return-type facts from typed callee
  bodies, annotate typed direct-call nodes with those facts after call-emission
  rewrites, preserve those facts across refresh, and then scope the unary-`not`
  compact-int fast path to exact-int operands.
- rejected result: `work/bench/knlskolznnxw_3498d5fa3eda`
  - specialized apply median: `586683 loops/s` (`-0.12%`)
  - verify pass: `330140 loops/s`
  - no-refcount diagnostic median: `745427 loops/s` (`-3.25%`)
  - latest summarized pystone code size: `59828 bytes`, `3587` machine blocks
  - code-size delta: `-483 bytes`, `-22` machine blocks
- reason rejected: return-fact annotation recovered most of the production
  throughput while shrinking code, but it still did not beat the kept baseline,
  and both verify and no-refcount diagnostics regressed. Keep the simpler
  unconditional unary-`not` fast path until return facts unlock a clear speed
  win elsewhere.
- validation before rejection: `just fmt-rust soac_opt soac_jit` passed;
  `cargo check -p soac_jit --tests` passed; `just benchmark` produced the
  rejected result above. The experiment was then reverted.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Require exact-int facts before unary `not` fast path

- baseline: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: the kept unary-`not` compact-int fast path adds guard/fallback
  code to every generic `not`. The existing truthiness emitter already handles
  known `None`, bool singleton, and exact-bool facts without a C helper call, so
  requiring exact-int facts looked like a way to keep the pystone win with less
  code growth.
- attempted change: only emit the compact-int unary-`not` fast path when
  `py_facts_for_arg(arg)` reports exact `int`; otherwise use the existing
  truthiness-to-bool path directly.
- rejected result: `work/bench/knlskolznnxw_cbf1c4dac21e`
  - specialized apply median: `573272 loops/s` (`-2.41%`)
  - verify pass: `331549 loops/s`
  - no-refcount diagnostic median: `748558 loops/s` (`-2.84%`)
  - latest summarized pystone code size: `59828 bytes`, `3587` machine blocks
  - code-size delta: `-483 bytes`, `-22` machine blocks
- reason rejected: the code-size reduction came from disabling the fast path at
  the hot `not Func2(...)` / `not Func3(...)` sites too. Those direct-call
  return values are not precise exact-int facts at generic unary emission time,
  so the narrower condition loses the measured unary-`not` throughput win.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `just benchmark` produced the rejected result
  above. The experiment was then reverted.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Generic exact-int rich-compare fast path

- baseline: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: hot CLIF still contained generic `PyObject_RichCompare` calls in
  `Proc0`, `Proc1`, and `Func2`, including comparisons of pystone integer
  results and constants that opt-v3 does not currently extract into an
  `I64CompareToBool01` plan.
- attempted change: teach generic comparison BinOps to guard both operands as
  exact compact `int`, unbox to `i64`, materialize the Python bool result, and
  fall back to the existing `PyObject_RichCompare` path on any guard miss.
- rejected result: `work/bench/knlskolznnxw_dfc473a823e7`
  - specialized apply median: `581161 loops/s` (`-1.06%`)
  - verify pass: `334874 loops/s`
  - no-refcount diagnostic median: `770311 loops/s` (`-0.02%`)
  - latest summarized pystone code size: `62031 bytes`, `3711` machine blocks
  - code-size delta: `+1720 bytes`, `+102` machine blocks
- reason rejected: the fast path removed none of the surrounding truthiness and
  fallback structure, while adding substantial guard code to comparison-heavy
  functions. Production apply and verify both regressed; the no-refcount
  diagnostic was neutral, so the change is not worth the code-size cost.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `just benchmark` produced the rejected result
  above. The experiment was then reverted.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Inline compact-int fast path for unary `not`

- baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s`
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- observation: `Proc0` still executes `BoolGlob = not Func2(...)`, where the
  guarded direct call to `Func2` returns pystone's `TRUE`/`FALSE` integer
  globals. The generic unary-not path calls through Python truthiness even when
  the returned object is an exact compact `int`.
- kept change: for `UnaryOpKind::Not`, emit a local fast path that guards the
  operand as an exact compact `PyLong`, compares the unboxed value against zero,
  materializes the corresponding Python bool, and falls back to the existing
  generic truthiness path for all other objects.
- first result: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `589777 loops/s` (`+1.25%`)
  - verify pass: `337276 loops/s`
  - no-refcount diagnostic median: `767311 loops/s` (`+0.15%`)
- confirmation result: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s` (`+0.84%`)
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s` (`+0.56%`)
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
  - code-size delta: `+499 bytes`, `+23` machine blocks
- reason kept: the production apply median repeated above the baseline, verify
  improved in both runs, and the no-refcount diagnostic stayed flat to slightly
  positive. The tradeoff is a modest `Proc0`/`Proc6` code-size increase from the
  extra guard/fallback blocks.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `just benchmark` produced the two results above.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Scalar threading into local-vs-local int comparisons

- baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s`
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- observation: `Proc0` still materializes `IntLoc1 = IntLoc1 + 1` as a
  Python object before the following `IntLoc1 < IntLoc2` loop test. Existing
  scalar-thread planning only recognized local-vs-constant comparisons, while
  this hot pystone shape is local-vs-local.
- attempted change: extend scalar-thread planning to recognize a stored scalar
  consumed by a local-vs-local exact-int branch, and teach the fused JIT emitter
  to preload only the non-threaded consumer operands.
- rejected result: `work/bench/knlskolznnxw_f393c0c16b52`
  - specialized apply median: `577616 loops/s` (`-0.84%`)
  - verify pass: `336290 loops/s`
  - no-refcount diagnostic median: `763003 loops/s` (`-0.42%`)
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- reason rejected: the focused planner unit test passed, but the production
  pystone typed IR still did not attach a scalar-thread plan to the `IntLoc1`
  loop store. Code size and specialization counters were unchanged, so this did
  not create a real pystone fast-path change.
- validation before rejection: `just fmt-rust soac_opt soac_jit` passed;
  `cargo test -p soac_opt plans_scalar_thread_for_store_rhs_followed_by_local_compare -- --nocapture`
  passed; `cargo check -p soac_jit --tests` passed; `just benchmark` produced
  the rejected result above. The experiment was then reverted.
- next baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`

## 2026-04-26 - Constant `i64` PythonLong materialization via module constants

- baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s`
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- observation: rendered hot CLIF still contained `PyLong_FromLongLong` calls for
  constant `i64` values such as `0` and `1` in comparison materialization
  paths. Those constants already exist as immortal module constants, so loading
  the module constant looked like a cheap way to avoid a CPython allocation
  helper call.
- attempted change: track known constant `i64` values inside v3 mechanical
  values, and when `MaterializeKind::PythonLong` sees such a constant already
  present in the module constant pool, emit the module constant pointer instead
  of calling `PyLong_FromLongLong`.
- rejected result: `work/bench/knlskolznnxw_accc6a432225`
  - specialized apply median: `574368 loops/s` (`-1.39%`)
  - verify pass: `323480 loops/s`
  - no-refcount diagnostic median: `746881 loops/s` (`-2.52%`)
  - latest summarized pystone code size: `59828 bytes`, `3584` machine blocks
- reason rejected: both production apply and the no-refcount diagnostic
  regressed, and generated code grew by `16` bytes despite two fewer machine
  blocks. The call removal does not pay for the different constant-load/code
  shape in this benchmark.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `just benchmark` produced the rejected result
  above. The experiment was then reverted.
- next baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`

## 2026-04-26 - Tstate-aware recursive-call check shim

- baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s`
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- observation: the accepted deep profile attributed about `2.49%` self time to
  `Py_EnterRecursiveCall` and another `2.12%` TLS lookup below it. Generated
  direct-call code already passes the current thread state to
  `dp_jit_enter_recursive_call`, but the helper ignored it and called the public
  `Py_EnterRecursiveCall` API.
- attempted change: add a tiny C shim that calls CPython's internal
  `_Py_EnterRecursiveCallTstate(tstate, ...)`, then have
  `dp_jit_enter_recursive_call` use that shim when generated code supplies a
  non-null thread state.
- rejected result: `work/bench/knlskolznnxw_24d3ef7476b2`
  - specialized apply median: `582232 loops/s` (`-0.04%`)
  - verify pass: `320027 loops/s`
  - no-refcount diagnostic median: `700448 loops/s` (`-8.58%`)
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- reason rejected: the refcount-enabled production result was a tie/slight
  regression, code size and counters were unchanged, and the no-refcount
  diagnostic regressed sharply. Avoid adding C build plumbing for no stable
  production win.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `just benchmark` produced the rejected result
  above. The experiment was then reverted.
- next baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`

## 2026-04-26 - Post-constructor-revert baseline recheck

- baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s`
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- result: `work/bench/knlskolznnxw_766a9b0a0795`
  - specialized apply median: `576396 loops/s` (`-1.05%`)
  - verify pass: `325724 loops/s`
  - no-refcount diagnostic median: `762663 loops/s` (`-0.46%`)
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- observation: the reverted source produced the same code-size and counter
  totals as the accepted iter baseline, but the median did not beat it. Treat
  this as a noise/current-state recheck, not a new baseline.
- next baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`

## 2026-04-26 - Straight-line constructor field-store inlining

- baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s`
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- observation: hot `Record` constructor calls allocate an instance and then run
  `Record.__init__`, whose typed body stores incoming arguments into indexed
  fields and returns `None`. Inlining those field stores after allocation looked
  like it could remove the direct `__init__` call and the constructor finish
  helper from the hot path.
- first attempted change: add a conservative typed detector for one-block
  constructor bodies made only of `SetAttrTyped` operations, then emit trusted
  indexed field stores directly after allocation with a cold fallback to the
  existing constructor call path.
- first result: `work/bench/knlskolznnxw_41254e1b9f82`
  - specialized apply median: `580517 loops/s` (`-0.34%`)
  - verify pass: `312759 loops/s`
  - no-refcount diagnostic median: `759407 loops/s` (`-0.89%`)
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- follow-up observation: the actual typed `Record.__init__` body uses
  temporary `Store`/`Del` aliases around each `SetAttrTyped`, so the first
  detector likely did not match and produced no structural win.
- second attempted change: extend the detector to resolve those temporary
  aliases and accept a statically `None` return.
- second result: `work/bench/knlskolznnxw_e660a9d45785`
  - profile pass: `269234 loops/s`
  - verify pass: crashed with `Segmentation fault (core dumped)`, recipe exit
    `139`, before writing a production apply summary.
- reason rejected: the strict detector did not beat baseline and showed no
  code-size change; the alias-aware detector reached the hot path but crashed in
  verify, so this needs a typed-plan-level design before it is safe to revisit.
- validation: `just fmt-rust soac_jit` and `cargo check -p soac_jit --tests`
  passed before the crashing benchmark. The experiment was reverted after the
  verify crash.
- next baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`

## 2026-04-26 - Bypass constructor init-result helper for statically None `__init__`

- baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s`
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- observation: direct constructor calls still call `dp_jit_finish_constructor_init`
  after a successful direct `__init__` call, even when the typed target can
  prove all return terms are runtime `None`. Pystone has hot `Record`
  constructor direct calls, so skipping that helper looked like a low-risk
  micro-optimization.
- attempted change: for typed direct constructor targets whose return terms are
  all runtime `None`, bypass `dp_jit_finish_constructor_init` after the existing
  null-result exception check and return the allocated object directly.
- first result: `work/bench/knlskolznnxw_85a34ae6290f`
  - specialized apply median: `599935 loops/s` (`+3.00%`)
  - verify pass: `341142 loops/s`
  - no-refcount diagnostic median: `756550 loops/s` (`-1.26%`)
- confirmation result:
  `work/bench/constructor-init-none-confirm/knlskolznnxw_85a34ae6290f_confirm1`
  - specialized apply median: `589945 loops/s` (`+1.28%`)
  - verify pass: `339573 loops/s`
  - no-refcount diagnostic median: `701326 loops/s` (`-8.46%`)
- second confirmation result:
  `work/bench/constructor-init-none-confirm/knlskolznnxw_85a34ae6290f_confirm2`
  - specialized apply median: `581479 loops/s` (`-0.17%`)
  - verify pass: `327938 loops/s`
  - no-refcount diagnostic median: `765237 loops/s` (`-0.13%`)
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- reason rejected: the first two refcount-enabled medians looked promising, but
  the second confirmation fell just below the accepted baseline. The generated
  code-size and counter summaries were unchanged, and the no-refcount diagnostic
  was noisy rather than supportive, so the change did not have enough structural
  evidence to keep.
- validation: `cargo check -p soac_jit --tests` passed before and after the
  experiment. `tests/test_regression_direct_exception_cleanup.py` was not a
  useful validator because its constructor-failure case panicked while binding a
  constructor type symbol for a path that the test expects not to use the v3
  constructor fast path.
- next baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`

## 2026-04-26 - Recheck Cranelift `speed` after iter/range work

- baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s`
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- observation: the generated pystone shape changed after the earlier
  `speed_and_size` default was chosen, so the old `speed` versus
  `speed_and_size` result deserved a cheap rerun.
- adjacent default recheck:
  `work/bench/current-default-after-range-revert/knlskolznnxw_37b33d7a021b`
  - Cranelift opt level: `speed_and_size`
  - specialized apply median: `570111 loops/s`
  - verify pass: `335796 loops/s`
  - no-refcount diagnostic median: `759689 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- first `speed` result:
  `work/bench/opt-level-speed-rerun-latest/knlskolznnxw_37b33d7a021b`
  - specialized apply median: `594598 loops/s` (`+2.08%`)
  - verify pass: `335146 loops/s`
  - no-refcount diagnostic median: `763387 loops/s` (`-0.37%`)
- confirmation `speed` result:
  `work/bench/opt-level-speed-rerun-latest-confirm/knlskolznnxw_37b33d7a021b`
  - specialized apply median: `581517 loops/s` (`-0.17%`)
  - verify pass: `331888 loops/s`
  - no-refcount diagnostic median: `765818 loops/s` (`-0.05%`)
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- reason rejected: the first `speed` run was strong, but the confirmation
  median did not beat the accepted iter baseline and no-refcount was flat.
  Code size was unchanged, so there was no structural reason to change the
  default based on one noisy run.
- validation: three `just benchmark` runs produced the default recheck and two
  `speed` results above. No code change was kept.
- next baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`

## 2026-04-26 - Fixed-arity `range` runtime primitives

- baseline: `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s`
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s`
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- observation: after keeping the `iter(x)` runtime primitive, deep profiling of
  the accepted result still attributed most residual `py_vectorcall_hook`
  samples to `range_vectorcall`; `PyObject_GetIter` had fallen to about `0.17%`.
- attempted change: add static direct runtime primitives for `range(stop)` and
  `range(start, stop)`, preserving CPython semantics by calling the CPython
  range type from `soac_jit_runtime`.
- first result: `work/bench/knlskolznnxw_eea699cb627e`
  - specialized apply median: `592433 loops/s` (`+1.71%`)
  - verify pass: `328238 loops/s`
  - no-refcount diagnostic median: `770086 loops/s` (`+0.51%`)
- confirmation result:
  `work/bench/range-runtime-primitive-confirm/knlskolznnxw_eea699cb627e`
  - specialized apply median: `578516 loops/s` (`-0.68%`)
  - verify pass: `329823 loops/s`
  - no-refcount diagnostic median: `766461 loops/s` (`+0.03%`)
  - latest summarized pystone code size: `59777 bytes`, `3588` machine blocks
- reason rejected: the first run looked promising, but the confirmation
  production median fell below the accepted iter baseline. The no-refcount
  diagnostic was effectively flat and generated machine blocks grew slightly,
  so there was no stable production win to keep.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo fmt
  --manifest-path crates/soac_jit_runtime/Cargo.toml` passed; `cargo check
  --manifest-path crates/soac_jit_runtime/Cargo.toml` passed; `cargo check -p
  soac_jit --tests` passed; `cargo test -p soac_jit direct_abi --
  --nocapture` passed; `cargo test -p soac_jit
  runtime_clif_builtin_primitive_symbols_are_available -- --nocapture` passed;
  `just pytest-fast tests/test_runtime_builtin_primitives.py -q` passed; both
  benchmark runs produced the rejected results above. The experiment was then
  removed.
- next baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`

## 2026-04-26 - Constructor direct calls

- baseline: `work/bench/knlskolznnxw_d7886af93322`
  - specialized apply median: `257595 loops/s`
  - verify pass: `173389 loops/s`
  - no-refcount diagnostic median: `229369 loops/s`
  - latest summarized pystone code size: `55082 bytes`, `3335` machine blocks
  - counters: `call_direct=1515002`, `call_hot_targets=2222189`,
    `runtime_decref=8163213`, `runtime_incref=6957036`
- observation: pystone recorded hot constructor targets that were not being
  selected as direct calls. `Record.copy` instruction `#0` observed
  `Record.__init__` about `101000` times, but had zero direct-call hits.
- attempted changes:
  - `work/bench/knlskolznnxw_aba33b91defe`: rejected, verify failed because
    the constructor callable relocation was not registered.
  - `work/bench/knlskolznnxw_05a927fe7685`: rejected, verify failed because
    the process-JIT reserved declaration snapshot did not predeclare the
    constructor callable relocation.
  - `work/bench/knlskolznnxw_8368080b1b19`: rejected, verify failed because
    the constructor allocation helper symbol was not registered.
- kept change: allow v3 profiled `__init__` targets to build direct-call
  argument plans with implicit `self`, resolve owner-type constructor guards
  during typed JIT planning, predeclare their relocation imports, and emit the
  existing guarded constructor allocation/init codegen path.
- result: `work/bench/knlskolznnxw_cce33b6d4c48`
  - specialized apply median: `266689 loops/s` (`+3.53%`)
  - verify pass: `176096 loops/s`
  - no-refcount diagnostic median: `236544 loops/s` (`+3.13%`)
  - latest summarized pystone code size: `57569 bytes`, `3472` machine blocks
  - counters: `call_direct=1616006` (`+101004`), `call_hot_targets=2222189`,
    `runtime_decref=8163213`, `runtime_incref=6957036`
- next lead: method calls still show profile evidence without direct-call hits.
  In particular, `Proc1` observes `Record.copy` at its hot method-call site but
  remains on the generic call path.

## 2026-04-26 - Method direct calls

- baseline: `work/bench/knlskolznnxw_cce33b6d4c48`
  - specialized apply median: `266689 loops/s`
  - verify pass: `176096 loops/s`
  - no-refcount diagnostic median: `236544 loops/s`
  - latest summarized pystone code size: `57569 bytes`, `3472` machine blocks
  - counters: `call_direct=1616006`, `field_access=3030081`,
    `runtime_decref=8163213`
- observation: `Proc1` instruction `#1` observed `Record.copy` about `101000`
  times through `call_hot_targets`, but still had zero direct-call hits before
  method-call callee kind was represented in v3 plan/emission data.
- kept change: extend v3 direct-call plan/emission data with an explicit callee
  kind (`Function`, `Method`, or `Constructor`), allow constant-name method
  sources to build direct-call argument plans with implicit receiver, resolve
  owner-type method guards during typed JIT planning, and predeclare their
  owner-attribute callable relocations for process JIT.
- result: `work/bench/knlskolznnxw_424bb3dcabb0`
  - specialized apply median: `273261 loops/s` (`+2.46%` vs constructor-only,
    `+6.08%` vs starting baseline)
  - verify pass: `176441 loops/s`
  - no-refcount diagnostic median: `245272 loops/s` (`+3.69%`)
  - latest summarized pystone code size: `57953 bytes`, `3498` machine blocks
  - counters: `call_direct=1717006` (`+101000`), `field_access=2929081`
    (`-101000`), `runtime_decref=8062213` (`-101000`)
  - key site: `Proc1` instruction `#1` now has `call_direct hit:101000,
    fallback:0` for `Record.copy`
- next lead: remaining zero-hit hot calls are mostly non-SOAC/runtime callables
  or unsupported shapes; inspect the largest remaining `call_hot_targets` and
  generic field fallbacks before trying another direct-call family.

## 2026-04-26 - External runtime direct-call targets

- baseline: `work/bench/knlskolznnxw_424bb3dcabb0`
  - specialized apply median: `273261 loops/s`
  - verify pass: `176441 loops/s`
  - no-refcount diagnostic median: `245272 loops/s`
  - latest summarized pystone code size: `57953 bytes`, `3498` machine blocks
  - counters: `call_direct=1717006`, `field_access=2929081`,
    `runtime_decref=8062213`
- observation: pystone still had hot external targets from `soac.runtime`,
  notably `range.__init__` and `exception_matches`, that were visible in
  `call_hot_targets` but unavailable to the runtime planner's single-module
  direct-call target index.
- attempted change: let the runtime v3 planner build its direct-call target
  index from retained compile-session modules that also appeared in the counter
  dump, so the current module could select direct calls into `soac.runtime`.
- rejected result: `work/bench/knlskolznnxw_030778e43d47`
  - specialized apply median: `193672 loops/s` (`-29.12%`)
  - verify pass: `130830 loops/s`
  - no-refcount diagnostic median: `181006 loops/s`
  - latest summarized pystone code size: `62502 bytes`, `3776` machine blocks
  - counters: `call_direct=1919016`, `field_access=3232095`,
    `runtime_decref=9476297`
- reason rejected: direct-calling external runtime helpers turned source-backed
  runtime operations such as `range.__init__` and `exception_matches` into JIT
  function bodies with substantially more emitted refcount work and larger
  pystone code, while still leaving the expensive builtin `iter`/`next` path in
  place.

## 2026-04-26 - Slotted runtime range helpers

- baseline: `work/bench/knlskolznnxw_424bb3dcabb0`
  - specialized apply median: `273261 loops/s`
  - verify pass: `176441 loops/s`
  - no-refcount diagnostic median: `245272 loops/s`
  - latest summarized pystone code size: `57953 bytes`, `3498` machine blocks
- observation: deep profiling showed a large remaining cost under generic
  Python iteration for the transformed `range` loop in `Proc8`; direct-calling
  runtime source helpers made this worse, so the next narrow experiment was to
  reduce the object attribute overhead inside the existing runtime helper
  classes.
- kept change: add fixed `__slots__` to `soac.runtime.range` and
  `soac.runtime.IterRange` for their existing fields.
- result: `work/bench/knlskolznnxw_f73276a42e82`
  - specialized apply median: `275603 loops/s` (`+0.86%`)
  - verify pass: `177496 loops/s`
  - no-refcount diagnostic median: `250454 loops/s` (`+2.11%`)
  - latest summarized pystone code size: `57953 bytes`, `3498` machine blocks
  - counters: `call_direct=1717006`, `field_access=2929081`,
    `runtime_decref=8062213`
- next lead: `Proc8` still goes through the generic `iter`/`next` protocol; a
  guarded direct helper for exact `soac.runtime.IterRange` may avoid the Python
  frame for `IterRange.__next__` without compiling broader runtime modules.

## 2026-04-26 - Guarded `IterRange.__next__` helper

- baseline: `work/bench/knlskolznnxw_f73276a42e82`
  - specialized apply median: `275603 loops/s`
  - verify pass: `177496 loops/s`
  - no-refcount diagnostic median: `250454 loops/s`
- observation: `Proc8` still spends time in generic `next(iterator)` over the
  transformed runtime `IterRange`, so a guarded typed-codegen helper looked
  like a narrow alternative to direct-calling the full `runtime.py` method.
- attempted change: guard `next(x)` on exact `soac.runtime.IterRange` and call a
  Rust helper that reads `current`, `stop`, and `step`, updates `current`, and
  returns the old value, falling back to normal iterator semantics for mutated
  or non-small-int fields.
- rejected results:
  - `work/bench/knlskolznnxw_e5579f4e795d`: profile pass failed because the
    reserved process-JIT declaration snapshot did not predeclare the
    `soac.runtime.IterRange` type relocation.
  - `work/bench/knlskolznnxw_3b03ff303119`: after adding the predeclare, the
    profile pass hung in pystone and was terminated with `SIGTERM`.
- reason rejected: this path is not a safe narrow optimization yet. The hang
  indicates the helper changed iterator progress or exception behavior in the
  hot `for range(...)` lowering path, so it was reverted instead of debugged
  further in the overnight loop.

## 2026-04-26 - Cranelift `speed_and_size` default

- baseline: `work/bench/knlskolznnxw_f73276a42e82`
  - specialized apply median: `275603 loops/s`
  - verify pass: `177496 loops/s`
  - no-refcount diagnostic median: `250454 loops/s`
- observation: the largest remaining generated functions are still `Proc0` and
  `Proc8`, and the no-refcount diagnostic continues to suggest pressure from
  emitted code size and refcount-heavy code shape. Cranelift's
  `speed_and_size` policy was a cheap tuning candidate.
- results:
  - `SOAC_CRANELIFT_OPT_LEVEL=speed_and_size` run:
    `work/bench/knlskolznnxw_4c0c9ca11a49`, specialized apply median
    `278702 loops/s`, verify `176707 loops/s`, no-refcount diagnostic
    `250986 loops/s`.
  - adjacent default `speed` rerun on the same tree: specialized apply median
    `272928 loops/s`, verify `178998 loops/s`, no-refcount diagnostic
    `252163 loops/s`.
  - second `speed_and_size` run: specialized apply median `277380 loops/s`,
    verify `176085 loops/s`, no-refcount diagnostic `251707 loops/s`.
- kept change: make normal runtime and benchmark Cranelift optimization default
  to `speed_and_size`, while keeping correctness recipes on `none` unless the
  caller overrides `SOAC_CRANELIFT_OPT_LEVEL`.
- default-change validation: `work/bench/knlskolznnxw_303454602431`
  - specialized apply median: `276598 loops/s` (`+0.36%` vs slotted range
    baseline)
  - verify pass: `181311 loops/s`
  - no-refcount diagnostic median: `250143 loops/s`

## 2026-04-26 - Profiled cold-block hints

- baseline: `work/bench/knlskolznnxw_303454602431`
  - specialized apply median: `276598 loops/s`
  - verify pass: `181311 loops/s`
  - no-refcount diagnostic median: `250143 loops/s`
  - latest summarized pystone code size: `57953 bytes`, `3498` machine blocks
- observation: the current code shape still has many small blocks, so replaying
  profile `block_entry` counters as Cranelift cold-block hints was a cheap
  existing-layout experiment.
- rejected result: `work/bench/knlskolznnxw_4df60310336b`
  - specialized apply median: `276537 loops/s` (`-0.02%`)
  - verify pass: `150204 loops/s`
  - no-refcount diagnostic median: `252032 loops/s`
  - latest summarized pystone code size: `58096 bytes`, `3519` machine blocks
- reason rejected: production apply throughput was flat within noise, verify
  throughput regressed sharply, and the extra block-entry counters slightly grew
  emitted code.

## 2026-04-26 - Guarded raw indices for exact-list item access

- baseline: `work/bench/knlskolznnxw_303454602431`
  - specialized apply median: `276598 loops/s`
  - verify pass: `181311 loops/s`
  - no-refcount diagnostic median: `250143 loops/s`
  - latest summarized pystone code size: `57953 bytes`, `3498` machine blocks
- observation: `Proc8` has hot exact-list getitem/setitem plans, but typed
  codegen still materialized the index as a PyLong and then immediately
  re-checked/unboxed that PyLong before direct list access.
- kept change: for planned exact-list item accesses whose index is a pure
  local/constant load or add/sub/mul of those, emit a guarded raw `i64` index
  for the fast path and materialize the original Python key only in the local
  generic fallback.
- result: `work/bench/knlskolznnxw_67a732a5dee8`
  - specialized apply median: `278007 loops/s` (`+0.51%`)
  - verify pass: `177572 loops/s`
  - no-refcount diagnostic median: `250574 loops/s` (`+0.17%`)
  - latest summarized pystone code size: `57998 bytes`, `3489` machine blocks
  - counters: `getitem_specialized=808002`, `setitem_specialized=707002`,
    `runtime_decref=8062213`
- next lead: the win is small and `Proc8` code size grew, so the remaining
  `iter`/`next` runtime path and the large `Proc0` refcount footprint are still
  better candidates than broadening item-index lowering.

## 2026-04-26 - Exact-string compare-to-bool branches

- baseline: `work/bench/knlskolznnxw_67a732a5dee8`
  - specialized apply median: `278007 loops/s`
  - verify pass: `177572 loops/s`
  - no-refcount diagnostic median: `250574 loops/s`
  - latest summarized pystone code size: `57998 bytes`, `3489` machine blocks
  - key remaining counter: `operator_hot_shapes=3939007`
- observation: `Func2` still compared string arguments through generic
  `PyObject_RichCompare` plus truthiness even when profile evidence showed
  exact `str`/`str` operands in branch context.
- attempted change:
  - `work/bench/knlskolznnxw_5858ea84598f`: rejected, verify failed because
    the first exact-string plan declared a raising operation before the local
    fallback, which the current mechanical lowering rejects.
- kept change: extend operator shape tags with exact `str`, derive exact-string
  planner facts from `operator_hot_shapes`, plan guarded exact-unicode branch
  comparisons as `PyObject_RichCompareBool`, and fall back to generic
  richcompare plus truthiness on guard miss.
- result: `work/bench/knlskolznnxw_007870c2fb53`
  - specialized apply median: `281575 loops/s` (`+1.28%`)
  - verify pass: `179464 loops/s`
  - no-refcount diagnostic median: `252815 loops/s` (`+0.89%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: `+216 bytes`, `+8` machine blocks
  - key site: specialized `Func2` instruction `#4` now carries exact-unicode
    guards and a `PyObjectRichCompareBool { op: Gt }` hot operation with a
    local generic fallback.
- next lead: refresh the deep profile on this result to see whether rich
  comparison time moved enough to make the remaining generic iteration path or
  `Proc0` refcount/code-size pressure the next best target.

## 2026-04-26 - Exact-int binary returns with constant operands

- baseline: `work/bench/knlskolznnxw_007870c2fb53`
  - specialized apply median: `281575 loops/s`
  - verify pass: `179464 loops/s`
  - no-refcount diagnostic median: `252815 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - key counter: `operator_hot_shapes=3535007`
- observation: after exact-string compare specialization, the deep profile
  still showed generic `PyNumber_Add`/`PyNumber_Subtract` time. Several hot
  exact-int operations were in shapes like `x + 5`, but the v3 exact-int
  materialization planner only accepted two loadable operands, not one local
  plus one constant.
- attempted change: plan compact exact-int binary materialization for one
  loadable operand plus one profiled integer constant, using checked machine
  arithmetic and PythonLong materialization on the hot path with the original
  generic operation as local fallback.
- rejected result: `work/bench/knlskolznnxw_d46be038da91`
  - specialized apply median: `279067 loops/s` (`-0.89%`)
  - verify pass: `180256 loops/s`
  - no-refcount diagnostic median: `255059 loops/s` (`+0.89%`)
  - latest summarized pystone code size: `59594 bytes`, `3602` machine blocks
  - code-size delta: `+1380 bytes`, `+105` machine blocks
  - counter delta: `operator_hot_shapes` dropped to `2727007`
- reason rejected: the extra guarded arithmetic plans removed generic
  operator counter work, but the refcount-enabled production path regressed
  and the code-size growth was large relative to the no-refcount diagnostic
  win.

## 2026-04-26 - Native iterator for runtime range

- baseline: `work/bench/knlskolznnxw_007870c2fb53`
  - specialized apply median: `281575 loops/s`
  - verify pass: `179464 loops/s`
  - no-refcount diagnostic median: `252815 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the refreshed deep profile still spent most non-JIT time in
  CPython's generic iteration stack: `builtin_next`, `PyObject_GetIter`,
  `_PyFunction_Vectorcall`, and runtime `IterRange.__next__` Python frames.
  The earlier typed-codegen `IterRange.__next__` helper was unsafe, so the
  narrow runtime alternative was to keep the reusable transformed `range`
  wrapper but return CPython's native range iterator from `range.__iter__`.
- kept change: `soac.runtime.range.__iter__` now delegates to
  `_builtins.iter(_builtins.range(self.start, self.stop, self.step))`, avoiding
  Python-level `IterRange.__next__` frames in hot transformed loops.
- result: `work/bench/knlskolznnxw_0594fd70afc8`
  - specialized apply median: `323300 loops/s` (`+14.82%`)
  - verify pass: `197153 loops/s`
  - no-refcount diagnostic median: `332353 loops/s` (`+31.46%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - counters and code size are unchanged from the exact-string baseline; the
    win comes from runtime iteration avoiding Python frame dispatch.
- validation: `just pytest-fast tests/test_runtime_builtin_primitives.py -q`
  passed, including reuse, index conversion, error behavior, and native
  `range_iterator` type assertion.
- next lead: rerun deep profiling on this new baseline; if the old iteration
  stack is gone, the next likely targets are `Proc0`/`Proc1` JIT code shape,
  indexed field helper calls, or remaining generic rich/number operations.

## 2026-04-26 - Native `range` object for transformed builtin range

- baseline: `work/bench/knlskolznnxw_0594fd70afc8`
  - specialized apply median: `323300 loops/s`
  - verify pass: `197153 loops/s`
  - no-refcount diagnostic median: `332353 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: after native iterator delegation, deep profiling still showed
  residual `_PyObject_MakeTpCall` / Python frame cost around the transformed
  runtime `range` wrapper. The wrapper was also less CPython-compatible than a
  native `range` object.
- kept change: re-export CPython's native `builtins.range` from `soac.runtime`
  and delete the Python-level runtime `range` wrapper, while leaving the legacy
  `IterRange` class in place for now.
- result: `work/bench/knlskolznnxw_df30feda23b6`
  - specialized apply median: `402103 loops/s` (`+24.38%`)
  - verify pass: `234988 loops/s`
  - no-refcount diagnostic median: `435211 loops/s` (`+30.95%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - counters and pystone code size are unchanged from the native-iterator
    baseline; the win comes from avoiding the transformed runtime wrapper object
    and its Python-level `__iter__` dispatch.
- validation: `just pytest-fast tests/test_runtime_builtin_primitives.py
  tests/test_regression_import_hook_broad_mode.py -q` passed, including native
  `builtins.range` type behavior and the adjusted runtime metadata assertion.
- next lead: refresh the deep profile on this new baseline; the remaining hot
  work should now be mostly generated pystone code, indexed-field helpers, and
  generic rich/number operations rather than runtime range dispatch.

## 2026-04-26 - Direct Unicode compare helper for exact-string branches

- baseline: `work/bench/knlskolznnxw_df30feda23b6`
  - specialized apply median: `402103 loops/s`
  - verify pass: `234988 loops/s`
  - no-refcount diagnostic median: `435211 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the native-range-object deep profile still showed
  `PyObject_RichCompare` / `PyObject_RichCompareBool` / `PyUnicode_RichCompare`
  time, and the exact-string branch plan was still calling
  `PyObject_RichCompareBool` after exact `PyUnicode_Type` guards.
- attempted change: add a distinct `PyUnicodeCompareBool` planned operation
  backed by a raw `soac_runtime_unicode_compare_bool` helper that calls
  `PyUnicode_Compare` directly after the existing exact-unicode guards.
- rejected result: `work/bench/knlskolznnxw_d7a540507ace`
  - specialized apply median: `396186 loops/s` (`-1.47%`)
  - verify pass: `230069 loops/s`
  - no-refcount diagnostic median: `432185 loops/s` (`-0.70%`)
  - latest summarized pystone code size: `58502 bytes`, `3520` machine blocks
  - code-size delta: `+288 bytes`, `+23` machine blocks
- reason rejected: direct Unicode comparison removed one generic dispatch layer,
  but the extra runtime helper path and code growth did not pay for itself in
  the refcount-enabled production benchmark.

## 2026-04-26 - Inline-values-only indexed field helpers

- baseline: `work/bench/knlskolznnxw_df30feda23b6`
  - specialized apply median: `402103 loops/s`
  - verify pass: `234988 loops/s`
  - no-refcount diagnostic median: `435211 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the native-range-object deep profile still spent about 8% of
  samples in `soac_runtime_store_field_indexed` and
  `soac_runtime_probe_field_indexed`. Pystone's hot field sites are exact
  owner-type/version guarded and use CPython inline-values slots, so the full
  helper's materialized-dict path is unnecessary on the common path.
- attempted change: add inline-values-only probe/store helpers that keep the
  same key/index checks, return a normal helper miss for materialized dicts or
  invalid inline-values blocks, and let the existing typed fallback execute
  generic CPython attribute access.
- variant result: `work/bench/knlskolznnxw_1f60dc96e11c`
  - specialized apply median: `410025 loops/s` (`+1.97%`)
  - verify pass: `235541 loops/s`
  - no-refcount diagnostic median: `442714 loops/s` (`+1.72%`)
  - latest summarized pystone code size: `64936 bytes`, `3924` machine blocks
  - code-size delta: `+6722 bytes`, `+427` machine blocks
  - decision: not kept in this form because runtime-support inlining made the
    throughput win expensive in generated code size.
- kept change: call the same inline-values-only helpers, but leave them out of
  the runtime-support inliner so each hot field site remains a compact helper
  call.
- result: `work/bench/knlskolznnxw_56cd074115e0`
  - specialized apply median: `408849 loops/s` (`+1.68%`)
  - verify pass: `229717 loops/s`
  - no-refcount diagnostic median: `430784 loops/s` (`-1.02%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
  - note: the third refcount-enabled run was an outlier at `367002 loops/s`;
    the first two runs were `411689` and `408849 loops/s`.
- validation: `cargo check -p soac_jit --tests` passed for the kept variant.
- next lead: refresh the deep profile on the kept field-helper result to see
  whether field helper samples moved enough for `Proc0`/`Proc1` code shape,
  remaining rich comparisons, or refcount traffic to become the next target.

## 2026-04-26 - Pointer-identity field-key check

- baseline: `work/bench/knlskolznnxw_56cd074115e0`
  - specialized apply median: `408849 loops/s`
  - verify pass: `229717 loops/s`
  - no-refcount diagnostic median: `430784 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the kept inline-values field helpers still call the shared
  `dict_key_matches` path, which can fall through to `PyObject_RichCompareBool`
  even though pystone's hot shared-key attribute layout should usually be an
  exact key pointer match.
- attempted change: make only the inline-values-only probe/store helpers require
  `indexed_key(keys, index) == key`, falling back to generic CPython attribute
  access when the cached key pointer is not identical.
- rejected result: `work/bench/knlskolznnxw_0cde07b68e9d`
  - specialized apply median: `399113 loops/s` (`-2.38%` versus the kept
    field-helper baseline)
  - verify pass: `233252 loops/s`
  - no-refcount diagnostic median: `448628 loops/s` (`+4.14%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: the no-refcount diagnostic improved, but the
  refcount-enabled production median regressed below both the kept helper result
  and the pre-field-helper baseline, so the narrower key check is not a useful
  production tradeoff.

## 2026-04-26 - One-argument `next` runtime primitive

- baseline: `work/bench/knlskolznnxw_56cd074115e0`
  - specialized apply median: `408849 loops/s`
  - verify pass: `229717 loops/s`
  - no-refcount diagnostic median: `430784 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: after replacing the transformed runtime `range` wrapper with
  CPython's native `range`, the deep profile still showed residual
  `builtin_next` / `py_vectorcall_hook` time in the hot iteration path.
- attempted change: add a static one-argument `next(x)` runtime primitive that
  checks `PyIter_Check`, calls the iterator's `tp_iternext` slot directly, and
  sets `StopIteration` when an exhausted iterator returns null without an
  exception.
- rejected result: `work/bench/knlskolznnxw_36dbd0f12506`
  - specialized apply median: `408650 loops/s` (`-0.05%` versus the kept
    field-helper baseline)
  - verify pass: `231310 loops/s`
  - no-refcount diagnostic median: `443781 loops/s` (`+3.02%`)
  - latest summarized pystone code size: `58155 bytes`, `3508` machine blocks
  - counter/code-shape note: `call_hot_targets` dropped from `2222189` to
    `1717133`, and total code size shrank by `59 bytes`, but machine blocks
    grew by `11`.
- reason rejected: the primitive did change the intended call shape and helped
  the no-refcount diagnostic, but the refcount-enabled production median was
  flat/slightly worse, so it is not a successful production optimization.

## 2026-04-26 - Store-only inline-values field helper inlining

- baseline: `work/bench/knlskolznnxw_56cd074115e0`
  - specialized apply median: `408849 loops/s`
  - verify pass: `229717 loops/s`
  - no-refcount diagnostic median: `430784 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the deep profile showed
  `soac_runtime_store_field_indexed_inline_values` at `5.29%` and
  `soac_runtime_probe_field_indexed_inline_values` at `3.63%`; inlining both
  helpers had produced a small speed win but added `6722 bytes` and `427`
  machine blocks.
- attempted change: add only
  `soac_runtime_store_field_indexed_inline_values` to the runtime-support
  inliner and keep the probe helper out of line.
- rejected result: `work/bench/knlskolznnxw_db3a22434f84`
  - specialized apply median: `400965 loops/s` (`-1.93%` versus the kept
    field-helper baseline)
  - verify pass: `230993 loops/s`
  - no-refcount diagnostic median: `452358 loops/s` (`+5.01%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: the production median regressed and the helper did not
  change the summarized JIT code size, so the likely benefit is again isolated
  to the unsound no-refcount diagnostic rather than the production path.

## 2026-04-26 - Effect-only direct-call inline candidates

- baseline: `work/bench/knlskolznnxw_56cd074115e0`
  - specialized apply median: `408849 loops/s`
  - verify pass: `229717 loops/s`
  - no-refcount diagnostic median: `430784 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: typed direct-call inlining only handled calls whose result was
  stored, so hot procedure-shaped calls such as `Proc5()` and `Proc4()` in
  `Proc0` still used guarded direct-call emission rather than the typed inline
  rewrite.
- attempted change: treat bare block-body `GuardedCallableCallTyped` nodes as
  effect-only inline candidates, restricted at planning time to callees whose
  explicit returns are runtime `None`, and route their ignored result through a
  temporary that is deleted in the cleanup block.
- rejected result: `work/bench/knlskolznnxw_d58c662a73cd`
  - specialized apply median: `400960 loops/s` (`-1.93%` versus the kept
    field-helper baseline)
  - verify pass: `239124 loops/s`
  - no-refcount diagnostic median: `436335 loops/s` (`+1.29%`)
  - latest summarized pystone code size: `58630 bytes`, `3525` machine blocks
  - code-size delta: `+416 bytes`, `+28` machine blocks
- reason rejected: the extra inline guard and cleanup shape improved verify and
  the no-refcount diagnostic, but the production refcount-enabled median
  regressed and generated code grew.

## 2026-04-26 - Direct-call target return facts for effect-only calls

- baseline: `work/bench/knlskolznnxw_56cd074115e0`
  - specialized apply median: `408849 loops/s`
  - verify pass: `229717 loops/s`
  - no-refcount diagnostic median: `430784 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: hot guarded direct calls such as `Proc5()` and `Proc4()` in
  `Proc0` target functions that explicitly return `None`, but the guarded
  direct-call result merge still discarded the result with unknown PyObject
  facts.
- attempted change: derive all-`None` return facts from typed direct-call target
  functions, use those facts when discarding effect-only direct-call results,
  and keep generic fallback results on the existing unknown-facts path.
- rejected result: `work/bench/knlskolznnxw_83c98b4795bf`
  - specialized apply median: `405309 loops/s` (`-0.87%` versus the kept
    field-helper baseline)
  - verify pass: `236550 loops/s`
  - no-refcount diagnostic median: `450351 loops/s` (`+4.54%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: the change helped verify and the no-refcount diagnostic, but
  the refcount-enabled production apply median regressed, so the added block
  shape was not worth keeping.

## 2026-04-26 - Skip refcount-call plumbing for `Unbound` locals

- baseline: `work/bench/knlskolznnxw_56cd074115e0`
  - specialized apply median: `408849 loops/s`
  - verify pass: `229717 loops/s`
  - no-refcount diagnostic median: `430784 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: `Unbound` locals already do not need transient decrefs, but the
  shared local refcount-call predicate still treated them as needing
  null-checked INCREF/DECREF plumbing.
- attempted change: make `local_ref_kind_needs_refcount_call` return false for
  both `Immortal` and `Unbound`.
- rejected result: `work/bench/knlskolznnxw_8794f8754ae9`
  - specialized apply median: `408823 loops/s` (`-0.01%` versus the kept
    field-helper baseline)
  - verify pass: `244231 loops/s`
  - no-refcount diagnostic median: `447313 loops/s` (`+3.84%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: verify improved, but the production refcount-enabled apply
  median was flat/slightly lower and generated code size was unchanged.

## 2026-04-26 - Return materialized `None` as immortal

- baseline: `work/bench/knlskolznnxw_56cd074115e0`
  - specialized apply median: `408849 loops/s`
  - verify pass: `229717 loops/s`
  - no-refcount diagnostic median: `430784 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: `emit_none_for_demand` explicitly emitted `INCREF(None)` and
  returned an owned PyObject result even though typed facts already model
  `None` as an immortal runtime singleton.
- kept change: return `EmitResult::immortal_pyobject` for materialized `None`
  and let legacy local-store validation accept immortal values as satisfying an
  owned PyObject demand.
- kept result: `work/bench/knlskolznnxw_0bfb9c7ebf5e`
  - specialized apply median: `411819 loops/s` (`+0.73%` versus the previous
    kept field-helper baseline)
  - verify pass: `232823 loops/s`
  - no-refcount diagnostic median: `452970 loops/s` (`+5.15%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- next baseline: `work/bench/knlskolznnxw_0bfb9c7ebf5e`

## 2026-04-26 - Remove remaining JIT-module `INCREF(None)` forwarding sites

- baseline: `work/bench/knlskolznnxw_0bfb9c7ebf5e`
  - specialized apply median: `411819 loops/s`
  - verify pass: `232823 loops/s`
  - no-refcount diagnostic median: `452970 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: after `emit_none_for_demand` was fixed, several lower-level
  block-argument, counter-result, indexed-setattr, and raise paths still
  explicitly emitted `INCREF(None)`.
- attempted change: remove those explicit `INCREF(None)` calls from
  `soac_jit/src/jit/mod.rs` and treat the no-expression raise placeholder as
  immortal.
- rejected result: `work/bench/knlskolznnxw_8f047ead6beb`
  - specialized apply median: `407781 loops/s` (`-0.98%` versus the immortal
    `None` baseline)
  - verify pass: `235527 loops/s`
  - no-refcount diagnostic median: `450795 loops/s` (`-0.48%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: unlike the centralized materialized-`None` change, removing
  these forwarding-site calls regressed production throughput and did not reduce
  summarized code size.

## 2026-04-26 - Use `PyUnicode_Equal` for indexed field key checks

- baseline: `work/bench/knlskolznnxw_0bfb9c7ebf5e`
  - specialized apply median: `411819 loops/s`
  - verify pass: `232823 loops/s`
  - no-refcount diagnostic median: `452970 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: deep-profile samples still put indexed field load/store helpers
  and `PyObject_RichCompare` near the top of the run.
- attempted change: keep the existing pointer equality fast path for split-dict
  cached keys, but replace the fallback `PyObject_RichCompareBool(..., Py_EQ)`
  call with `PyUnicode_Equal` because attribute keys are expected to be Unicode.
- rejected result: `work/bench/knlskolznnxw_381c691586c9`
  - specialized apply median: `406102 loops/s` (`-1.39%` versus the immortal
    `None` baseline)
  - verify pass: `243440 loops/s`
  - no-refcount diagnostic median: `441623 loops/s` (`-2.51%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: verify improved, but both production apply and the
  no-refcount diagnostic regressed, so the more specialized Unicode comparison
  did not pay off for pystone.

## 2026-04-26 - Resolve Python C-API JIT symbols directly

- baseline: `work/bench/knlskolznnxw_0bfb9c7ebf5e`
  - specialized apply median: `411819 loops/s`
  - verify pass: `232823 loops/s`
  - no-refcount diagnostic median: `452970 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: deep-profile attributed self time to thin forwarding wrappers
  around hot Python C-API calls, including `pyobject_richcompare_wrapper` and
  `pynumber_add_wrapper`, in addition to the forwarded C implementation cost.
- kept change: when registering Cranelift JIT symbols, resolve the CPython
  C-API function with `dlsym(RTLD_DEFAULT, ...)` once and register that direct
  address, falling back to the existing wrapper only if symbol resolution fails.
- kept result: `work/bench/knlskolznnxw_aac5e5d73cce`
  - specialized apply median: `414928 loops/s` (`+0.75%` versus the immortal
    `None` baseline)
  - verify pass: `241903 loops/s`
  - no-refcount diagnostic median: `440233 loops/s` (`-2.81%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- next baseline: `work/bench/knlskolznnxw_aac5e5d73cce`

## 2026-04-26 - Return cached keys from inline-values field check

- baseline: `work/bench/knlskolznnxw_aac5e5d73cce`
  - specialized apply median: `414928 loops/s`
  - verify pass: `241903 loops/s`
  - no-refcount diagnostic median: `440233 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the latest deep profile still showed
  `soac_runtime_store_field_indexed_inline_values` and
  `soac_runtime_probe_field_indexed_inline_values` as notable helper costs.
  Both helpers reloaded the object type and cached keys immediately after the
  shared inline-values eligibility check had already loaded the type.
- attempted change: have the inline-values eligibility helper return the cached
  keys with the values pointer, keeping the same guard, key-match, and helper
  miss behavior.
- rejected result: `work/bench/knlskolznnxw_594a3f29972a`
  - specialized apply median: `412255 loops/s` (`-0.64%` versus the direct
    C-API symbol baseline)
  - verify pass: `235632 loops/s`
  - no-refcount diagnostic median: `459461 loops/s` (`+4.37%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: reducing the helper reloads helped the no-refcount
  diagnostic but regressed the refcount-enabled production median, so the
  small helper-shape change was reverted.

## 2026-04-26 - Exact compact-ASCII `ord()` helper fast path

- baseline: `work/bench/knlskolznnxw_aac5e5d73cce`
  - specialized apply median: `414928 loops/s`
  - verify pass: `241903 loops/s`
  - no-refcount diagnostic median: `440233 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the refreshed perf profile still showed the hot
  `chr(ord(CharIndex)+1)` path going through `PyUnicode_GetLength`,
  `PyUnicode_ReadChar`, and `PyUnicode_FromOrdinal`. The current `ord()`
  runtime primitive always called the public Unicode C API even for exact
  one-character compact ASCII strings.
- kept change: add an exact `PyUnicode_Type` compact-ASCII fast path in
  `soac_runtime_builtin_ord_i64` that reads the one-byte payload directly,
  while preserving the existing C-API fallback for subclasses, non-ASCII
  strings, and non-compact forms.
- kept result: `work/bench/knlskolznnxw_c5bb52b51205`
  - specialized apply median: `415620 loops/s` (`+0.17%` versus the direct
    C-API symbol baseline)
  - verify pass: `241783 loops/s`
  - no-refcount diagnostic median: `457775 loops/s` (`+3.99%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- adjacent rerun: `work/bench/ord_fast_rerun/knlskolznnxw_c5bb52b51205`
  - specialized apply median: `415877 loops/s`
  - verify pass: `242397 loops/s`
  - no-refcount diagnostic median: `459295 loops/s`
- validation: `just pytest-fast tests/test_runtime_builtin_primitives.py -q`
  passed; `cargo check -p soac_jit --tests` passed after formatting the
  standalone runtime crate.
- next baseline: `work/bench/ord_fast_rerun/knlskolznnxw_c5bb52b51205`

## 2026-04-26 - Production trivial-jump threading

- baseline: `work/bench/ord_fast_rerun/knlskolznnxw_c5bb52b51205`
  - specialized apply median: `415877 loops/s`
  - verify pass: `242397 loops/s`
  - no-refcount diagnostic median: `459295 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the refreshed Proc0 VCode profile showed many hot blocks that
  were just refcount plumbing and jump-only merge blocks. SOAC already had a
  post-opt trivial-jump threading pass for inspection output, but production
  compilation did not run it before backend codegen.
- attempted change: run the existing post-opt trivial-jump threading pass on
  the production Cranelift function before CFG/domtree recomputation,
  verification, and backend compilation.
- rejected result: `work/bench/knlskolznnxw_ffd9bd5d6aa0`
  - specialized apply median: `401330 loops/s` (`-3.50%` versus the compact
    ASCII `ord()` baseline)
  - verify pass: `239598 loops/s`
  - no-refcount diagnostic median: `452000 loops/s` (`-1.59%`)
  - latest summarized pystone code size: `58422 bytes`, `3425` machine blocks
  - code-size delta: `+208 bytes`, `-72` machine blocks
- reason rejected: the pass removed machine blocks, but it increased byte size
  and significantly regressed production throughput, so the production hook was
  reverted.

## 2026-04-26 - Exact compact-int `PyNumber_Add`/`Subtract` helper wrappers

- baseline: `work/bench/ord_fast_rerun/knlskolznnxw_c5bb52b51205`
  - specialized apply median: `415877 loops/s`
  - verify pass: `242397 loops/s`
  - no-refcount diagnostic median: `459295 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the refreshed deep profile still showed `PyNumber_Add` and
  `PyNumber_Subtract` in the hot path after the direct CPython C-API symbol
  change. Pystone's arithmetic is mostly exact compact `int` arithmetic, so a
  wrapper could bypass CPython's generic numeric slot dispatch without changing
  generated CLIF.
- attempted change: register custom JIT helpers for `PyNumber_Add` and
  `PyNumber_Subtract` that directly decode exact compact `PyLong` operands,
  return `PyLong_FromLongLong` for non-overflowing `i64` results, and fall back
  to CPython for subclasses, non-compact longs, overflow, and non-int inputs.
- rejected result: `work/bench/knlskolznnxw_4dafe45eea34`
  - specialized apply median: `406318 loops/s` (`-2.30%` versus the compact
    ASCII `ord()` baseline)
  - verify pass: `244646 loops/s` (`+0.93%`)
  - no-refcount diagnostic median: `435922 loops/s` (`-5.09%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: the helper work improved the verification pass but regressed
  both production apply and the no-refcount diagnostic. Replacing the direct
  CPython C-API entry with a wrapper adds enough call/helper overhead to lose
  overall, so the wrapper was reverted.

## 2026-04-26 - Pointer-identity shortcut for exact Unicode comparisons

- baseline: `work/bench/ord_fast_rerun/knlskolznnxw_c5bb52b51205`
  - specialized apply median: `415877 loops/s`
  - verify pass: `242397 loops/s`
  - no-refcount diagnostic median: `459295 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the deep profile still showed `PyObject_RichCompareBool`,
  `PyObject_RichCompare`, and Unicode `memcmp` costs. For exact Unicode
  comparisons, two identical object pointers determine the result for all six
  rich-compare operators without calling the C API.
- attempted change: in the v3 exact-Unicode `PyObject_RichCompareBool`
  mechanical emission, branch around the C-API call when `lhs == rhs`, returning
  true for `==`, `<=`, and `>=`, and false for `!=`, `<`, and `>`.
- rejected result: `work/bench/knlskolznnxw_cd8adb478362`
  - specialized apply median: `414612 loops/s` (`-0.30%` versus the compact
    ASCII `ord()` baseline)
  - verify pass: `226457 loops/s` (`-6.58%`)
  - no-refcount diagnostic median: `450457 loops/s` (`-1.92%`)
  - latest summarized pystone code size: `58238 bytes`, `3499` machine blocks
  - code-size delta: `+24 bytes`, `+2` machine blocks
- reason rejected: the shortcut was semantically safe but added branch/code-size
  overhead to every exact-Unicode compare while only catching a small subset of
  pystone comparisons, so it was reverted.

## 2026-04-26 - `dp_jit_is_true` singleton fast path

- baseline: `work/bench/ord_fast_rerun/knlskolznnxw_c5bb52b51205`
  - specialized apply median: `415877 loops/s`
  - verify pass: `242397 loops/s`
  - no-refcount diagnostic median: `459295 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: `PyObject_IsTrue` still appeared in the deep profile, and
  generated truthiness already goes through SOAC's `dp_jit_is_true` helper, so
  the helper could recognize singleton truth values before falling back to
  CPython without changing generated CLIF.
- attempted change: make `dp_jit_is_true` return directly for `True`, `False`,
  and `None`, preserving the existing `PyObject_IsTrue` fallback for all other
  values.
- first result: `work/bench/knlskolznnxw_3a5a878fe020`
  - specialized apply median: `416128 loops/s` (`+0.06%` versus the compact
    ASCII `ord()` baseline)
  - verify pass: `235943 loops/s` (`-2.66%`)
  - no-refcount diagnostic median: `457593 loops/s` (`-0.37%`)
- confirmation result: `work/bench/is_true_fast_rerun/knlskolznnxw_3a5a878fe020`
  - specialized apply median: `414750 loops/s` (`-0.27%`)
  - verify pass: `232220 loops/s` (`-4.20%`)
  - no-refcount diagnostic median: `461837 loops/s` (`+0.55%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: the only positive production result was within run-to-run
  noise and did not repeat. The helper fast path was reverted.

## 2026-04-26 - Exact compact-ASCII Unicode compare helper

- baseline: `work/bench/ord_fast_rerun/knlskolznnxw_c5bb52b51205`
  - specialized apply median: `415877 loops/s`
  - verify pass: `242397 loops/s`
  - no-refcount diagnostic median: `459295 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: Unicode rich comparisons still showed up through
  `PyObject_RichCompareBool`, `PyObject_RichCompare`, and `memcmp`. Pystone has
  many exact one-character ASCII comparisons, so a helper could answer those
  directly and fall back to CPython for full strings and unusual op ids.
- attempted change: route the v3 exact-Unicode compare operation to
  `soac_runtime_exact_unicode_compare_bool`, first as a local runtime-support
  CLIF helper and then as a normal JITBuilder helper symbol. The helper checked
  pointer equality and one-character compact-ASCII payloads before falling back
  to `PyObject_RichCompareBool`.
- unbenchmarked runtime-support failures:
  - `work/bench/knlskolznnxw_c874426cee6a` failed before measurement because
    the runtime CLIF retained a private helper function as an unresolved local.
  - `work/bench/knlskolznnxw_f2d9cd1aa346` and
    `work/bench/knlskolznnxw_29994cbc43e3` failed before measurement with a
    Cranelift module panic while loading the local runtime-support CLIF.
- benchmarked helper-symbol result: `work/bench/knlskolznnxw_506ec0bd8b26`
  - specialized apply median: `413239 loops/s` (`-0.63%` versus the compact
    ASCII `ord()` baseline)
  - verify pass: `249450 loops/s` (`+2.91%`)
  - no-refcount diagnostic median: `450193 loops/s` (`-1.98%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: the JITBuilder helper-symbol version loaded and improved
  verify, but it regressed the production apply median and no-refcount
  diagnostic. The local runtime-support helper shape also exposed loader
  fragility, so the compare helper was reverted.

## 2026-04-26 - Direct `PyObject_RichCompareBool` JIT symbol registration

- baseline: `work/bench/ord_fast_rerun/knlskolznnxw_c5bb52b51205`
  - specialized apply median: `415877 loops/s`
  - verify pass: `242397 loops/s`
  - no-refcount diagnostic median: `459295 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: the kept direct CPython C-API symbol registration covered
  `PyObject_RichCompare`, but `PyObject_RichCompareBool` was still an
  unregistered import and remained visible in the pystone deep profile.
- attempted change: register `PyObject_RichCompareBool` through the same
  direct-symbol lookup path, with a wrapper fallback only if the CPython symbol
  is unavailable.
- rejected result: `work/bench/knlskolznnxw_0e6447ee832d`
  - specialized apply median: `415731 loops/s` (`-0.04%` versus the compact
    ASCII `ord()` baseline)
  - verify pass: `241714 loops/s` (`-0.28%`)
  - no-refcount diagnostic median: `460407 loops/s` (`+0.24%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- reason rejected: the production median was slightly below the current
  baseline and the only improvement was in the unsound no-refcount diagnostic,
  so the registration was reverted.

## 2026-04-26 - Fast `next(range_iterator)` vectorcall

- baseline: `work/bench/ord_fast_rerun/knlskolznnxw_c5bb52b51205`
  - specialized apply median: `415877 loops/s`
  - verify pass: `242397 loops/s`
  - no-refcount diagnostic median: `459295 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: after native `builtins.range` and the compact-ASCII `ord()`
  fast path, the deep profile still showed residual `builtin_next` and
  `dp_jit_py_vectorcall` cost in the hot `for range(...)` paths.
- kept change: teach `dp_jit_py_vectorcall` to recognize one-argument
  `builtins.next` calls on exact CPython `range_iterator` objects, update the
  raw iterator fields directly, return `PyLong_FromLong`, and set
  `StopIteration` on exhaustion. All other `next` calls and non-range
  iterators still use the generic CPython vectorcall path.
- first result: `work/bench/knlskolznnxw_e1793f438ec7`
  - specialized apply median: `424268 loops/s` (`+2.02%`)
  - verify pass: `244518 loops/s`
  - no-refcount diagnostic median: `467693 loops/s` (`+1.83%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- confirmation result:
  `work/bench/range_next_fast_rerun/knlskolznnxw_e1793f438ec7`
  - specialized apply median: `422478 loops/s` (`+1.59%`)
  - verify pass: `243047 loops/s`
  - no-refcount diagnostic median: `465579 loops/s` (`+1.37%`)
  - code-size delta: unchanged
- validation: `cargo check -p soac_jit --tests` passed; `just pytest-fast
  tests/test_runtime_builtin_primitives.py -q` passed.
- next baseline:
  `work/bench/range_next_fast_rerun/knlskolznnxw_e1793f438ec7`

## 2026-04-26 - Fast one-argument `builtins.iter` vectorcall

- baseline:
  `work/bench/range_next_fast_rerun/knlskolznnxw_e1793f438ec7`
  - specialized apply median: `422478 loops/s`
  - verify pass: `243047 loops/s`
  - no-refcount diagnostic median: `465579 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: after the fast `next(range_iterator)` vectorcall change,
  `dp_jit_py_vectorcall` still spent measurable cumulative time around
  iterator setup through `builtins.iter`.
- attempted change: teach `dp_jit_py_vectorcall` to recognize exact
  one-argument calls to the cached `builtins.iter` object and call
  `PyObject_GetIter(arg)` directly, while preserving the generic vectorcall path
  for keywords, non-one-argument calls, and all other callables.
- rejected result: `work/bench/knlskolznnxw_4383d26b89dd`
  - specialized apply median: `422298 loops/s` (`-0.04%`)
  - verify pass: `238353 loops/s` (`-1.93%`)
  - no-refcount diagnostic median: `465303 loops/s` (`-0.06%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- validation: `just pytest-fast tests/test_runtime_builtin_primitives.py -q`
  passed before the benchmark.
- reason rejected: the production apply median was slightly below the kept
  baseline, and the hook added another branch to every vectorcall without
  removing enough runtime cost. The `iter` hook was reverted.

## 2026-04-26 - C-level `StopIteration` exception-match fast path

- baseline:
  `work/bench/range_next_fast_rerun/knlskolznnxw_e1793f438ec7`
  - specialized apply median: `422478 loops/s`
  - verify pass: `243047 loops/s`
  - no-refcount diagnostic median: `465579 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: after the fast `next(range_iterator)` vectorcall change, the
  deep profile still showed about `9.65%` cumulative time under
  `dp_jit_py_vectorcall -> _PyFunction_Vectorcall -> _PyEval_EvalFrameDefault`.
  The cached BlockPy/typed IR mapped that path to lowered `for`-loop exception
  matching: `__soac__.exception_matches(current_exception(), StopIteration)`.
- kept change: teach `dp_jit_py_vectorcall` to recognize exact calls to the
  cached `soac.runtime.exception_matches` function when the second argument is
  exactly `StopIteration`, and answer with `PyErr_GivenExceptionMatches` plus a
  Python bool result. All non-`StopIteration` exception matching still uses the
  Python helper, preserving the existing runtime validation path for arbitrary
  `except` expressions.
- first result: `work/bench/knlskolznnxw_d8bfb6a3f353`
  - specialized apply median: `471441 loops/s` (`+11.59%`)
  - verify pass: `260197 loops/s` (`+7.06%`)
  - no-refcount diagnostic median: `612085 loops/s` (`+31.47%`)
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- confirmation result:
  `work/bench/stop_iteration_match_rerun/knlskolznnxw_d8bfb6a3f353`
  - specialized apply median: `471471 loops/s` (`+11.60%`)
  - verify pass: `251242 loops/s` (`+3.37%`)
  - no-refcount diagnostic median: `619490 loops/s` (`+33.06%`)
  - code-size delta: unchanged
- validation: `cargo check -p soac_jit --tests` passed; `just pytest-fast
  tests/test_runtime_builtin_primitives.py -q` passed.
- next baseline:
  `work/bench/stop_iteration_match_rerun/knlskolznnxw_d8bfb6a3f353`

## 2026-04-26 - Trusted inline-values indexed field helpers

- baseline:
  `work/bench/stop_iteration_match_rerun/knlskolznnxw_d8bfb6a3f353`
  - specialized apply median: `471471 loops/s`
  - verify pass: `251242 loops/s`
  - no-refcount diagnostic median: `619490 loops/s`
  - latest summarized pystone code size: `58214 bytes`, `3497` machine blocks
- observation: after the `StopIteration` match fast path, the refreshed deep
  profile showed `soac_runtime_store_field_indexed_inline_values` at `7.20%`
  self and `soac_runtime_probe_field_indexed_inline_values` at `4.41%` self.
  The hot pystone field sites already have exact owner-type/version guards
  before entering the helper, but the helper still repeated the cached-key kind
  and attribute-key comparison.
- kept change: add trusted inline-values probe/store helper variants for
  typed indexed-field codegen. They still reject materialized instance dicts,
  invalid inline-values storage, and out-of-capacity indices, but rely on the
  already-emitted exact type/version guard for the selected field index instead
  of repeating the key-layout comparison inside the helper.
- first result: `work/bench/knlskolznnxw_ea3f9294f9dd`
  - specialized apply median: `494142 loops/s` (`+4.81%`)
  - verify pass: `258223 loops/s` (`+2.78%`)
  - no-refcount diagnostic median: `642152 loops/s` (`+3.66%`)
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
- confirmation result:
  `work/bench/trusted_field_helpers_rerun/knlskolznnxw_ea3f9294f9dd`
  - specialized apply median: `491701 loops/s` (`+4.29%`)
  - verify pass: `266000 loops/s` (`+5.87%`)
  - no-refcount diagnostic median: `648053 loops/s` (`+4.61%`)
  - code-size delta: `-408 bytes`, machine blocks unchanged
- validation: `cargo check -p soac_jit --tests` passed; `just pytest-fast
  tests/test_runtime_builtin_primitives.py -q` passed.
- next baseline:
  `work/bench/trusted_field_helpers_rerun/knlskolznnxw_ea3f9294f9dd`

## 2026-04-26 - Owned trusted indexed-field load helper

- baseline:
  `work/bench/trusted_field_helpers_rerun/knlskolznnxw_ea3f9294f9dd`
  - specialized apply median: `491701 loops/s`
  - verify pass: `266000 loops/s`
  - no-refcount diagnostic median: `648053 loops/s`
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
- observation: the trusted field-load helper still returned a borrowed slot
  value and then typed codegen emitted a separate INCREF helper call for the
  owned Python result. Combining the slot probe and INCREF into one helper was
  a plausible way to remove one generated helper call at hot field-load sites.
- attempted change: add a trusted owned-load helper that returns an already
  INCREF'd field value and remove the separate generated INCREF call from the
  indexed-field load fast path.
- rejected result: `work/bench/knlskolznnxw_ffb706b1ed32`
  - specialized apply median: `490559 loops/s` (`-0.23%`)
  - verify pass: `260210 loops/s` (`-2.18%`)
  - no-refcount diagnostic median: `644440 loops/s` (`-0.56%`)
  - latest summarized pystone code size: `57811 bytes`, `3497` machine blocks
  - `runtime_incref` counters dropped from `6957036` to `6452036`, confirming
    the code shape changed, but the production throughput did not improve.
- reason rejected: the extra helper shape slightly regressed the
  refcount-enabled median and grew code size, so it was reverted.
- next baseline:
  `work/bench/trusted_field_helpers_rerun/knlskolznnxw_ea3f9294f9dd`

## 2026-04-26 - Split trusted indexed-field store insert/overwrite branches

- baseline:
  `work/bench/trusted_field_helpers_rerun/knlskolznnxw_ea3f9294f9dd`
  - specialized apply median: `491701 loops/s`
  - verify pass: `266000 loops/s`
  - no-refcount diagnostic median: `648053 loops/s`
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
- observation: the trusted inline-values store helper remained hot at about
  `5.25%` self time. Verify counters showed roughly `1.62M` specialized field
  stores and `1.41M` specialized field loads, with both constructor
  first-inserts and `Proc1`/`Proc3` overwrites on the hot path. The helper
  still tested whether the old slot was null twice on every successful store.
- kept change: split the trusted helper's first-insert and overwrite bodies so
  the old-slot null branch is tested once, with the same fallback, insertion
  order, INCREF, store, and DECREF behavior as before.
- result: `work/bench/knlskolznnxw_6e1c59465c76`
  - specialized apply median: `495495 loops/s` (`+0.77%`)
  - verify pass: `273223 loops/s`
  - no-refcount diagnostic median: `647130 loops/s` (`-0.14%`)
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
  - code-size delta: unchanged
- validation: `cargo check -p soac_jit --tests` passed; `just pytest-fast
  tests/test_runtime_builtin_primitives.py -q` passed.
- next baseline: `work/bench/knlskolznnxw_6e1c59465c76`

## 2026-04-26 - Trusted inline-values flag-skip helper

- baseline: `work/bench/knlskolznnxw_6e1c59465c76`
  - specialized apply median: `495495 loops/s`
  - verify pass: `273223 loops/s`
  - no-refcount diagnostic median: `647130 loops/s`
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
- observation: the trusted inline-values field helpers already relied on typed
  codegen's exact owner type/version guard for key-layout validity, but still
  reused the generic inline-values extraction macro, including a repeated owner
  type flag check for `Py_TPFLAGS_INLINE_VALUES | Py_TPFLAGS_MANAGED_DICT`.
- attempted change: add a trusted inline-values extraction macro for the
  trusted load/store helpers that skipped the repeated owner type flag test
  while still rejecting materialized dictionaries and invalid inline-values
  storage.
- rejected result: `work/bench/knlskolznnxw_46638caef4e4`
  - specialized apply median: `485925 loops/s` (`-1.93%`)
  - verify pass: `255518 loops/s` (`-6.48%`)
  - no-refcount diagnostic median: `646968 loops/s` (`-0.03%`)
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
- reason rejected: the production apply-pass median regressed despite unchanged
  code size and flat no-refcount diagnostics, so the helper was reverted.
- next baseline: `work/bench/knlskolznnxw_6e1c59465c76`

## 2026-04-26 - Overwrite-first trusted indexed-field store helper

- baseline: `work/bench/knlskolznnxw_6e1c59465c76`
  - specialized apply median: `495495 loops/s`
  - verify pass: `273223 loops/s`
  - no-refcount diagnostic median: `647130 loops/s`
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
- observation: pystone's specialized field stores are a mix of constructor
  first-inserts and hot-loop overwrites, with overwrites likely dominating after
  initialization. The trusted store helper's split branch still laid out the
  first-insert path before the overwrite path.
- attempted change: reorder the trusted helper to handle non-null old slots
  first and return immediately after the overwrite store/decref path, leaving
  first-insert handling unchanged.
- rejected result: `work/bench/knlskolznnxw_5ce9d6eb9b38`
  - specialized apply median: `489928 loops/s` (`-1.12%`)
  - verify pass: `249406 loops/s` (`-8.72%`)
  - no-refcount diagnostic median: `650083 loops/s` (`+0.46%`)
- reason rejected: the refcount-enabled apply median and verify pass both
  regressed, so the original first-insert-first layout was restored.
- next baseline: `work/bench/knlskolznnxw_6e1c59465c76`

## 2026-04-26 - Unary-not direct-call inline shape

- baseline: `work/bench/knlskolznnxw_6e1c59465c76`
  - specialized apply median: `495495 loops/s`
  - verify pass: `273223 loops/s`
  - no-refcount diagnostic median: `647130 loops/s`
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
- observation: refreshed block attribution showed `Func2` prologue samples as
  the top JIT block. The hot call site in `Proc0` is `BoolGlob = not
  Func2(...)`, while the typed direct-call inliner only handles direct
  `target = call(...)` stores.
- attempted change: extend the inline candidate recognition and typed rewrite
  to handle a guarded direct call under unary `not`.
- rejected pre-benchmark result: rendering specialized `Proc0` showed no IR
  shape change. The `Func2` call remained a guarded direct call because the
  existing inline buildability gate still rejects the large callee body before
  the typed rewrite can apply.
- reason rejected: no specialized IR effect for the hot pystone call site, so
  the change was reverted without a benchmark run.
- next baseline: `work/bench/knlskolznnxw_6e1c59465c76`

## 2026-04-26 - Effect-only exact-list setitem result

- baseline: `work/bench/knlskolznnxw_6e1c59465c76`
  - specialized apply median: `495495 loops/s`
  - verify pass: `273223 loops/s`
  - no-refcount diagnostic median: `647130 loops/s`
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
- observation: pystone has `707002` specialized exact-list setitem counter
  samples in verify, with no deopts or guard failures. The typed statement path
  emitted the specialized list store and then materialized/incref'd `None` even
  when the result demand was effect-only.
- attempted change: add a typed effect-only setitem emission path that reused
  the existing exact-list guards and store logic, but jumped to a no-value
  continuation on a specialized hit instead of materializing an owned `None`.
- first result: `work/bench/knlskolznnxw_2d28bd390e0e`
  - specialized apply median: `498128 loops/s` (`+0.53%`)
  - verify pass: `265102 loops/s` (`-2.97%`)
  - no-refcount diagnostic median: `640443 loops/s` (`-1.03%`)
  - latest summarized pystone code size: `57865 bytes`, `3488` machine blocks
- confirmation result: `work/bench/recheck/knlskolznnxw_2d28bd390e0e`
  - specialized apply median: `495430 loops/s` (`-0.01%`)
  - verify pass: `253349 loops/s` (`-7.27%`)
  - no-refcount diagnostic median: `646134 loops/s` (`-0.15%`)
  - latest summarized pystone code size: `57865 bytes`, `3488` machine blocks
- reason rejected: the small first-run median improvement did not reproduce,
  while verify regressed and code size grew by `59 bytes`, so the codegen change
  was reverted.
- validation before revert: `cargo check -p soac_jit --tests` passed; `cargo
  test -p soac_jit exact_list -- --nocapture` passed; `just pytest-fast
  tests/test_runtime_builtin_primitives.py -q` passed.
- next baseline: `work/bench/knlskolznnxw_6e1c59465c76`

## 2026-04-26 - Exact Unicode branch compare via PyUnicode_Compare

- baseline: `work/bench/knlskolznnxw_6e1c59465c76`
  - specialized apply median: `495495 loops/s`
  - verify pass: `273223 loops/s`
  - no-refcount diagnostic median: `647130 loops/s`
  - latest summarized pystone code size: `57806 bytes`, `3497` machine blocks
- observation: exact-string branch optimization already guarded both operands
  as exact `str`, but the backend still lowered the selected operation through
  `PyObject_RichCompareBool`. The baseline perf profile still showed generic
  rich-compare work under pystone's hot string comparison path.
- kept change: lower selected exact-string branch comparisons to
  `PyUnicode_Compare` after the exact Unicode guards, then compare the signed
  result against zero for the requested rich-compare operator.
- result: `work/bench/knlskolznnxw_15b79ecc5275`
  - specialized apply median: `502048 loops/s` (`+1.32%`)
  - verify pass: `266435 loops/s`
  - no-refcount diagnostic median: `654695 loops/s` (`+1.17%`)
  - latest summarized pystone code size: `57798 bytes`, `3497` machine blocks
  - code-size delta: `-8 bytes`, machine blocks unchanged
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `cargo test -p soac_opt exact_str -- --nocapture` passed;
  `cargo test -p soac_jit
  specialized_jit_opt_v3_exact_str_branch_uses_unicode_compare -- --nocapture`
  passed; `just benchmark` produced the result above.
- next baseline: `work/bench/knlskolznnxw_15b79ecc5275`

## 2026-04-26 - Generic rich-compare wrapper exact-Unicode fast path

- baseline: `work/bench/knlskolznnxw_15b79ecc5275`
  - specialized apply median: `502048 loops/s`
  - verify pass: `266435 loops/s`
  - no-refcount diagnostic median: `654695 loops/s`
  - latest summarized pystone code size: `57798 bytes`, `3497` machine blocks
- observation: refreshed deep-profile output still showed
  `PyObject_RichCompare` at `4.63%` self time, with Unicode comparison and
  recursive-call/TLS overhead under it. Some of those calls come from generic
  comparison sites that are not yet selected as exact-string branch operations.
- attempted change: register SOAC's existing `PyObject_RichCompare` wrapper
  unconditionally and add an exact-Unicode fast path that calls
  `PyUnicode_Compare` and returns the corresponding `bool`, falling back to
  CPython's real `PyObject_RichCompare` for all other operand shapes.
- rejected result: `work/bench/knlskolznnxw_b6fd97f666fb`
  - specialized apply median: `488163 loops/s` (`-2.77%`)
  - verify pass: `257507 loops/s`
  - no-refcount diagnostic median: `617665 loops/s` (`-5.66%`)
  - latest summarized pystone code size: `57798 bytes`, `3497` machine blocks
- reason rejected: the wrapper indirection regressed both refcount-enabled and
  no-refcount apply medians without reducing generated code size, so the change
  was reverted. The remaining rich-compare work should be handled as selected
  typed operations instead of hidden in the generic C-API wrapper.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed before the benchmark; `just benchmark` produced the rejected
  result above.
- next baseline: `work/bench/knlskolznnxw_15b79ecc5275`

## 2026-04-26 - Generic PyNumber exact-compact-int helper

- baseline: `work/bench/knlskolznnxw_15b79ecc5275`
  - specialized apply median: `502048 loops/s`
  - verify pass: `266435 loops/s`
  - no-refcount diagnostic median: `654695 loops/s`
  - latest summarized pystone code size: `57798 bytes`, `3497` machine blocks
- observation: the refreshed perf profile still showed `PyNumber_Add` at
  `3.16%` self time, and rendered pystone CLIF still contained many
  `PyNumber_Add`, `PyNumber_Subtract`, and `PyNumber_Multiply` calls in hot
  functions such as `Proc0`, `Proc8`, and `Func2`.
- attempted change: route add/sub/mul imports through SOAC-named helper symbols
  that fast-path exact compact `int` operands with checked i64 arithmetic and
  `PyLong_FromLongLong`, falling back to the real CPython `PyNumber_*` C API for
  every other shape.
- rejected result: `work/bench/knlskolznnxw_d2d9a1c275fb`
  - specialized apply median: `489477 loops/s` (`-2.50%`)
  - verify pass: `257207 loops/s`
  - no-refcount diagnostic median: `653206 loops/s` (`-0.23%`)
  - latest summarized pystone code size: `57798 bytes`, `3497` machine blocks
- reason rejected: the helper-level type checks and wrapper indirection were
  slower than the generic CPython path for the refcount-enabled run and did not
  reduce generated code size. The remaining integer arithmetic work should be
  selected earlier as typed operations, not hidden inside a generic C-API helper.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `cargo test -p soac_jit
  render_specialized_jit_operator_calls_use_direct_number_helper -- --nocapture`
  passed; `cargo test -p soac_jit specialized_jit_opt_v3_exact_int --
  --nocapture` passed; `cargo test -p soac_jit
  specialized_jit_opt_v3_add_store_then_compare_constant_emits_machine_paths --
  --nocapture` passed; `just benchmark` produced the rejected result above.
- next baseline: `work/bench/knlskolznnxw_15b79ecc5275`

## 2026-04-26 - Inline trusted indexed-field load probe

- baseline: `work/bench/knlskolznnxw_15b79ecc5275`
  - specialized apply median: `502048 loops/s`
  - verify pass: `266435 loops/s`
  - no-refcount diagnostic median: `654695 loops/s`
  - latest summarized pystone code size: `57798 bytes`, `3497` machine blocks
- observation: the refreshed perf profile showed
  `soac_runtime_probe_field_indexed_inline_values_trusted` at `2.37%` self time,
  and pystone has `2929081` indexed-field access samples in verify. The helper
  was only used after an exact type/version guard had already selected an
  inline-values field layout.
- kept change: inline the trusted load probe in typed indexed-getattr codegen
  after the exact type/version guard, checking only materialized-dict absence,
  inline-values validity, slot capacity, and non-null value before jumping to
  the existing hit path. The now-unused trusted probe runtime helper and import
  symbol were removed.
- first result: `work/bench/knlskolznnxw_82157d84ae97`
  - specialized apply median: `511294 loops/s` (`+1.84%`)
  - verify pass: `274490 loops/s`
  - no-refcount diagnostic median: `659476 loops/s` (`+0.73%`)
  - latest summarized pystone code size: `58613 bytes`, `3554` machine blocks
- confirmation result after cleanup: `work/bench/knlskolznnxw_61ab3f5f439e`
  - specialized apply median: `510936 loops/s` (`+1.77%`)
  - verify pass: `267824 loops/s`
  - no-refcount diagnostic median: `640655 loops/s` (`-2.14%`)
  - latest summarized pystone code size: `58613 bytes`, `3554` machine blocks
  - code-size delta: `+815 bytes`, `+57` machine blocks
- noise note: one intervening confirmation run in the same result directory had
  a `498453 loops/s` median, but rerunning the unchanged cleaned revision
  returned to `510936 loops/s`; the generated code size and specialization
  counters were unchanged.
- validation: `just fmt-rust soac_jit` passed; `cargo fmt --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo check -p soac_jit --tests`
  passed; `cargo check --manifest-path crates/soac_jit_runtime/Cargo.toml`
  passed; `cargo test -p soac_jit
  field_index_specialized_getattr_hits_apply_mode_fast_path -- --nocapture`
  passed; `just benchmark` produced the confirmation result above.
- next baseline: `work/bench/knlskolznnxw_61ab3f5f439e`

## 2026-04-26 - Trusted indexed-field overwrite helper

- baseline: `work/bench/knlskolznnxw_61ab3f5f439e`
  - specialized apply median: `510936 loops/s`
  - verify pass: `267824 loops/s`
  - no-refcount diagnostic median: `640655 loops/s`
  - latest summarized pystone code size: `58613 bytes`, `3554` machine blocks
- observation: the post-load-probe perf profile moved
  `soac_runtime_store_field_indexed_inline_values_trusted` to the largest
  SOAC-specific runtime helper at `4.42%` self time.
- attempted change: inline the trusted indexed-field store layout checks in JIT
  codegen, call a new overwrite-only helper for already-filled slots, and keep
  the existing trusted store helper for first inserts.
- rejected result: `work/bench/knlskolznnxw_29a85095cff4`
  - specialized apply median: `504244 loops/s` (`-1.31%`)
  - verify pass: `262247 loops/s`
  - no-refcount diagnostic median: `668410 loops/s` (`+4.33%`)
  - latest summarized pystone code size: `60738 bytes`, `3700` machine blocks
- reason rejected: the refcount-enabled run regressed and code size grew
  substantially. The no-refcount diagnostic improved, which suggests the
  control-flow shape reduced pure store-helper overhead, but the production path
  paid too much in extra JIT layout checks, larger code, and refcount/helper
  call structure.
- validation: `just fmt-rust soac_jit` passed; `cargo fmt --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo check -p soac_jit --tests`
  passed; `cargo check --manifest-path crates/soac_jit_runtime/Cargo.toml`
  passed; `cargo test -p soac_jit
  field_index_specialized_setattr_hits_apply_mode_insert_and_overwrite --
  --nocapture` passed; `just benchmark` produced the rejected result above.
- next baseline: `work/bench/knlskolznnxw_61ab3f5f439e`

## 2026-04-26 - Ordering comparisons via PyObject_RichCompareBool

- baseline: `work/bench/knlskolznnxw_61ab3f5f439e`
  - specialized apply median: `510936 loops/s`
  - verify pass: `267824 loops/s`
  - no-refcount diagnostic median: `640655 loops/s`
  - latest summarized pystone code size: `58613 bytes`, `3554` machine blocks
- observation: the profile still showed `PyObject_RichCompare` at `4.24%` self
  time, and boolean comparisons in ordinary typed code materialized an object
  result before truth-testing it.
- attempted change: route only ordering comparisons (`<`, `<=`, `>`, `>=`) in
  boolean context through `PyObject_RichCompareBool`. Equality and inequality
  were deliberately left on the object-result path because CPython's bool helper
  has an identity shortcut for `==` and `!=` that can change user-visible
  `__eq__`/`__ne__` behavior.
- rejected result: `work/bench/knlskolznnxw_32ed321bc93e`
  - specialized apply median: `501096 loops/s` (`-1.93%`)
  - verify pass: `283951 loops/s`
  - no-refcount diagnostic median: `662950 loops/s` (`+3.48%`)
  - latest summarized pystone code size: `58557 bytes`, `3550` machine blocks
- reason rejected: the production apply run regressed even though verify,
  no-refcount, and code size improved. This likely traded away object allocation
  work for a CPython helper path that was slower in the refcount-enabled steady
  state.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `cargo test -p soac_jit
  specialized_jit_ordering_if_uses_richcomparebool -- --nocapture` passed;
  `just benchmark` produced the rejected result above.
- next baseline: `work/bench/knlskolznnxw_61ab3f5f439e`

## 2026-04-26 - Drop redundant trusted-store index checks

- baseline: `work/bench/knlskolznnxw_61ab3f5f439e`
  - specialized apply median: `510936 loops/s`
  - verify pass: `267824 loops/s`
  - no-refcount diagnostic median: `640655 loops/s`
  - latest summarized pystone code size: `58613 bytes`, `3554` machine blocks
- observation: `soac_runtime_store_field_indexed_inline_values_trusted` remained
  a hot helper and is called from JIT code with a constant non-negative field
  index. Its signed negative-index check and first-insert `index > u8::MAX`
  check are redundant after the capacity check.
- attempted change: remove those redundant index checks from the trusted
  inline-values store helper, leaving the debug assertion and capacity check.
- rejected result: `work/bench/knlskolznnxw_946fe39d9300`
  - specialized apply median: `499843 loops/s` (`-2.17%`)
  - verify pass: `267057 loops/s`
  - no-refcount diagnostic median: `647107 loops/s` (`+1.01%`)
  - latest summarized pystone code size: `58613 bytes`, `3554` machine blocks
- reason rejected: the generated code size was unchanged and the
  refcount-enabled median regressed. This helper is sensitive enough that small
  control-flow changes need full apply benchmarking, not just source-level
  simplification.
- validation: `cargo fmt --manifest-path crates/soac_jit_runtime/Cargo.toml`
  passed; `cargo check --manifest-path crates/soac_jit_runtime/Cargo.toml`
  passed; `cargo test -p soac_jit
  field_index_specialized_setattr_hits_apply_mode_insert_and_overwrite --
  --nocapture` passed; `just benchmark` produced the rejected result above.
- next baseline: `work/bench/knlskolznnxw_61ab3f5f439e`

## 2026-04-26 - Profiled cold-block hints

- baseline: `work/bench/knlskolznnxw_61ab3f5f439e`
  - specialized apply median: `510936 loops/s`
  - verify pass: `267824 loops/s`
  - no-refcount diagnostic median: `640655 loops/s`
  - latest summarized pystone code size: `58613 bytes`, `3554` machine blocks
- observation: the current pipeline has an opt-in
  `SOAC_ENABLE_PROFILED_COLD_BLOCKS` path that records block-entry counters and
  replays cold block hints into Cranelift during verify/apply.
- attempted change: benchmarked the existing path with
  `SOAC_ENABLE_PROFILED_COLD_BLOCKS=1` to decide whether it should become the
  default.
- rejected result: `work/bench/knlskolznnxw_fcecd04d1504`
  - specialized apply median: `505345 loops/s` (`-1.09%`)
  - verify pass: `215798 loops/s`
  - no-refcount diagnostic median: `657929 loops/s` (`+2.70%`)
  - latest summarized pystone code size: `58770 bytes`, `3575` machine blocks
- reason rejected: cold-block instrumentation and layout hints increased code
  size and slowed the refcount-enabled apply run. It is still useful as an
  opt-in diagnostic knob, but should not become the default for pystone.
- validation: `SOAC_ENABLE_PROFILED_COLD_BLOCKS=1 just benchmark` produced the
  rejected result above.
- next baseline: `work/bench/knlskolznnxw_61ab3f5f439e`

## 2026-04-26 - Skip global-load module-constant name decrefs

- baseline: `work/bench/knlskolznnxw_61ab3f5f439e`
  - specialized apply median: `510936 loops/s`
  - verify pass: `267824 loops/s`
  - no-refcount diagnostic median: `640655 loops/s`
  - latest summarized pystone code size: `58613 bytes`, `3554` machine blocks
- observation: the post-load perf mapping still showed hot generated DECREF
  blocks on values loaded from module-constant symbols. Global-load code passed
  an immortal module-constant name object to the fast/indexed/slow helpers, then
  emitted a normal owned-input DECREF for that same name object on the direct,
  fallback, and deopt paths.
- kept change: stop emitting those global-load name-object DECREFs. Module
  constants are immortalized when built, and the global-load helpers do not
  steal the name argument.
- accepted result: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s` (`+2.79%`)
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s` (`+1.50%`)
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- reason kept: the refcount-enabled apply median improved well beyond the
  recent noise range, and generated pystone code size shrank by `83` bytes and
  `58` machine blocks.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `just benchmark` produced the accepted result above.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Prefer guarded list-index lowering with planned item access

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation: `Proc8` still materialized hot exact-list indices such as
  `IntLoc + 7` through `PyNumber_Add` before entering exact-list item
  get/set specialization. The JIT already has a guarded i64-index lowering path
  for exact-list item plans, but verify/apply shape counters kept the older
  object-index path selected.
- attempted change: make exact-list item plans prefer the guarded i64-index
  path whenever the index expression can be guarded, even when a shape counter
  is configured for verify/apply.
- rejected result: `work/bench/knlskolznnxw_46009e0dbfe3`
  - specialized apply median: `520218 loops/s` (`-0.95%`)
  - verify pass: `305093 loops/s`
  - no-refcount diagnostic median: `645669 loops/s` (`-0.70%`)
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- reason rejected: verify improved substantially, but the production
  refcount-enabled apply median regressed and code size did not shrink. The
  extra guarded-index control flow appears to cost more than the avoided Python
  arithmetic for this pystone shape.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `just pytest-fast
  tests/test_counter_dump_file.py::test_getitem_v3_profile_replay_records_hit_and_fallback_counters
  tests/test_counter_dump_file.py::test_setitem_v3_profile_replay_records_hit_and_fallback_counters`
  passed; `just benchmark` produced the rejected result above.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Treat module-constant loads as borrowed inputs

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation: hot `Proc0` CLIF still passed module constants such as `1`, `5`,
  and `7` to generic Python numeric helpers and then emitted generated DECREF
  paths for those constant values.
- attempted change: treat `Load` from a module-constant location as a borrowed
  PyObject input in both Codegen and Typed emission.
- rejected first result: `work/bench/knlskolznnxw_8dd60309b85e`
  - specialized apply median: `531214 loops/s` (`+1.15%`)
  - verify pass: `289515 loops/s`
  - no-refcount diagnostic median: `675897 loops/s` (`+3.94%`)
  - latest summarized pystone code size: `58530 bytes`, `3493` machine blocks
- rejected confirmation result: `work/bench/confirm/knlskolznnxw_8dd60309b85e`
  - specialized apply median: `523126 loops/s` (`-0.39%`)
  - verify pass: `284538 loops/s`
  - no-refcount diagnostic median: `676284 loops/s` (`+4.00%`)
  - latest summarized pystone code size: `58530 bytes`, `3493` machine blocks
- reason rejected: the first run looked promising, but the confirmation run
  regressed the production refcount-enabled apply median. The no-ref diagnostic
  still improved, which suggests the idea reduces work but is lost in the
  normal refcount-enabled path's current bottlenecks.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `cargo test -p soac_jit
  module_constant_loads_are_borrowed_pyobject_inputs -- --nocapture` passed;
  two benchmark runs produced the rejected results above.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Repeat default Cranelift speed-and-size run

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation: pystone is now sensitive to generated code footprint. The
  current benchmark default is already Cranelift `speed_and_size`, so this was
  a repeat run of the default rather than a distinct code/config change.
- attempted change: none; repeat the existing default pipeline with
  `SOAC_CRANELIFT_OPT_LEVEL=speed_and_size` explicitly.
- rejected result:
  `work/bench/opt-level-speed-size/knlskolznnxw_5a4e523e739c`
  - specialized apply median: `526925 loops/s` (`+0.33%`)
  - verify pass: `281619 loops/s`
  - no-refcount diagnostic median: `651154 loops/s` (`+0.14%`)
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- reason rejected: the apply median moved by less than one percent, code size
  did not shrink, and the mean refcount-enabled run was essentially tied with
  the current baseline. This only updated the noise envelope; there was no
  code/config change to keep.
- validation: `SOAC_CRANELIFT_OPT_LEVEL=speed_and_size just benchmark 1000000
  100000 work/bench/opt-level-speed-size` produced the rejected result above.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Inline effect-only direct calls

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation: pystone has hot effect-only calls such as `Proc5()` and
  `Proc4()`, while the typed direct-call inliner only handled assignment-shaped
  calls like `x = f(...)`.
- attempted change: first teach the typed inliner to rewrite bare guarded
  callable call statements, then extend the v3 direct-call planner so bare
  lowered calls are marked as inline candidates.
- rejected recognizer-only result: `work/bench/knlskolznnxw_296bb0ce31cb`
  - specialized apply median: `523299 loops/s` (`-0.36%`)
  - verify pass: `280201 loops/s`
  - no-refcount diagnostic median: `653587 loops/s` (`+0.51%`)
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- rejected full-planner result: `work/bench/knlskolznnxw_a7f0b68cbdeb`
  - specialized apply median: `515844 loops/s` (`-1.78%`)
  - verify pass: `275838 loops/s`
  - no-refcount diagnostic median: `651333 loops/s` (`+0.17%`)
  - latest summarized pystone code size: `59002 bytes`, `3525` machine blocks
- reason rejected: once the planner actually enabled the inline arms, `Proc0`
  grew by `472` bytes and `29` machine blocks and the refcount-enabled apply
  median regressed sharply. The call overhead saved by inlining `Proc5`/`Proc4`
  is not enough to pay for the added guard/control-flow footprint in this shape.
- validation: `just fmt-rust soac_opt` passed; `cargo check -p soac_opt
  --tests` passed; `cargo test -p soac_opt inline -- --nocapture` passed;
  `cargo test -p soac_opt direct_call_inline_candidate_requires_buildable_inline_body
  -- --nocapture` passed; `cargo check -p soac_jit --tests` passed; two
  `just benchmark` runs produced the rejected results above.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Trim trusted indexed field-store checks

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation:
  `soac_runtime_store_field_indexed_inline_values_trusted` is still a roughly
  `5%` self-time hotspot in the deep profile.
- attempted change: in the trusted helper, load split-value capacity once and
  remove the redundant negative/u8 index checks that should be covered by the
  trusted caller contract and the capacity check.
- rejected result: `work/bench/knlskolznnxw_b64d84f2468d`
  - specialized apply median: `518432 loops/s` (`-1.29%`)
  - verify pass: `280470 loops/s`
  - no-refcount diagnostic median: `651768 loops/s` (`+0.23%`)
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- reason rejected: the generated pystone code shape did not change, and the
  refcount-enabled apply run regressed. The helper branch simplification was
  too small to overcome normal code layout or branch-prediction effects.
- validation: `cargo fmt --manifest-path crates/soac_jit_runtime/Cargo.toml`
  passed; `cargo check -p soac_jit --tests` passed; `just benchmark` produced
  the rejected result above.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Exact StopIteration match shortcut

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation: the current vectorcall hook already has a fast path for
  `soac.runtime.exception_matches(exc, StopIteration)`, but it still called
  `PyErr_GivenExceptionMatches` for every match. The benchmark profile continued
  to show exception matching in the residual iterator/StopIteration path.
- attempted change: short-circuit exact `StopIteration` type objects and exact
  `StopIteration` instances before falling back to `PyErr_GivenExceptionMatches`
  for subclass and unusual exception values.
- rejected result: `work/bench/knlskolznnxw_4bfb37f57191`
  - specialized apply median: `523178 loops/s` (`-0.38%`)
  - verify pass: `295242 loops/s`
  - no-refcount diagnostic median: `656600 loops/s` (`+0.98%`)
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- reason rejected: verify and the no-refcount diagnostic improved, but the
  refcount-enabled production median regressed below the accepted baseline. The
  extra branch in the vectorcall hook is not worth keeping for pystone.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `just pytest-fast tests/test_runtime_builtin_primitives.py
  -q` passed; `just benchmark` produced the rejected result above.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Dead local stores for unused range targets

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation: the hot pystone `for range(...)` loops still materialize loop
  target `PyLong` values, store them into locals, and release them even when the
  target is never read by the remaining block or successor. The existing typed
  refcount plan already marks these locals as released on the loop jump edge.
- attempted change: during typed JIT emission, detect local stores whose value
  is released on the same jump edge and whose target is not read by the
  remaining block body; emit the RHS for effects and leave the local unbound
  instead of emitting a store/release pair. Also let the one-argument positional
  call helper share the existing `next(range_iter)` fast path so effect-only
  calls do not fall back to generic builtin `next`.
- first result: `work/bench/knlskolznnxw_ce42b2f371de`
  - specialized apply median: `527803 loops/s` (`+0.50%`)
  - verify pass: `290229 loops/s`
  - no-refcount diagnostic median: `673663 loops/s` (`+3.60%`)
  - latest summarized pystone code size: `58498 bytes`, `3497` machine blocks
- confirmation result: `work/bench/confirm/knlskolznnxw_ce42b2f371de`
  - specialized apply median: `521900 loops/s` (`-0.63%`)
  - verify pass: `271818 loops/s`
  - no-refcount diagnostic median: `671137 loops/s` (`+3.21%`)
  - latest summarized pystone code size: `58498 bytes`, `3497` machine blocks
- rejected runner error: an intermediate confirmation command passed
  `results_root=work/bench/confirm` as the positional benchmark loop argument
  and failed before the apply pass; it was ignored and rerun with the correct
  Justfile argument order.
- reason rejected: the no-refcount diagnostic improved and the generated code
  got slightly smaller, but the refcount-enabled production median did not
  confirm. The extra dead-store detection/codegen shape is not worth keeping
  without a stable production apply win.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `just pytest-fast tests/test_runtime_builtin_primitives.py
  -q` passed; two usable `just benchmark` runs produced the rejected/confirmed
  results above.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Static direct-call env readiness without entry pointer

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation: the resolved direct-call emitter uses a statically declared
  Cranelift `func_ref` when the callee is part of the reserved direct-function
  batch, but it still loaded and checked the runtime direct-entry pointer before
  making that static call. The static call does not use the loaded entry
  pointer, though the function env still must have deopt/runtime metadata.
- attempted change: for static direct calls, replace the full direct-entry
  pointer readiness helper with a narrower function-env readiness check that
  verifies the deopt table and calls `dp_jit_direct_compile_function_env` only
  when the env is not ready. Keep the full entry-pointer path for indirect
  direct calls.
- rejected result: `work/bench/knlskolznnxw_3d62940f0fe1`
  - specialized apply median: `523888 loops/s` (`-0.25%`)
  - verify pass: `294678 loops/s`
  - no-refcount diagnostic median: `655866 loops/s` (`+0.86%`)
  - latest summarized pystone code size: `58142 bytes`, `3464` machine blocks
- reason rejected: the code got smaller and verify improved, but the
  refcount-enabled production apply median regressed below the accepted
  baseline. Removing the entry-pointer check is not enough to produce a stable
  pystone win.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `cargo test -p soac_jit direct_call -- --nocapture` passed;
  `just benchmark` produced the rejected result above.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Effect-only next primitive for deleted range-loop targets

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation: prior dead-store experiments improved the no-refcount diagnostic
  but still left `next(range_iterator)` materializing a `PyLong` for unused
  loop targets. A narrower approach could preserve ordinary `next()` semantics
  while using the direct runtime-primitive framework for calls whose result is
  provably effect-only.
- attempted change: add a `next` runtime primitive with `ResultAbi::NoValue`,
  implement `soac_runtime_builtin_next_effect` with an exact range-iterator raw
  advance path and generic `PyIter_Next` fallback, and add a typed JIT peephole
  for `tmp = next(iter); deleted_target = tmp` so the first store records an
  immortal `None` placeholder instead of a materialized item.
- rejected result: `work/bench/knlskolznnxw_1dd63b1cef11`
  - specialized apply median: `524439 loops/s` (`-0.14%`)
  - verify pass: `290589 loops/s`
  - no-refcount diagnostic median: `666537 loops/s` (`+2.51%`)
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- reason rejected: the no-refcount diagnostic and verify pass improved, but the
  refcount-enabled production apply median stayed below the accepted baseline.
  Avoiding item materialization here is not enough to overcome the remaining
  refcount/runtime costs in the production run.
- validation: `just fmt-rust soac_jit` passed; `cargo fmt --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo check -p soac_jit
  --tests` passed; `cargo check --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo test -p soac_jit
  runtime_primitive -- --nocapture` passed; `just pytest-fast
  tests/test_runtime_builtin_primitives.py -q` passed; `just benchmark`
  produced the rejected result above. After reverting the experiment source,
  `just fmt-rust soac_jit`, `cargo fmt --manifest-path
  crates/soac_jit_runtime/Cargo.toml`, and `cargo check -p soac_jit --tests`
  passed.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Delay indexed-field attribute operands to fallback

- baseline: `work/bench/knlskolznnxw_7d70b19fc78e`
  - specialized apply median: `525189 loops/s`
  - verify pass: `284975 loops/s`
  - no-refcount diagnostic median: `650252 loops/s`
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- observation: typed indexed-field get/set emits the attribute-name operand
  before the exact type/version guard, but the trusted inline-values helper
  path does not use that operand. Delaying it to the fallback path looked like
  a way to remove hot-path constant loads and possible cleanup work without
  changing the helper ABI.
- attempted change: for typed indexed getattr/setattr, emit only the receiver
  (and replacement for setattr) before the guard, remove the attribute operand
  from the guard-miss replay-safety set, and materialize the attribute only in
  the cold generic `PyObject_GetAttr` / `PyObject_SetAttr` fallback block.
- rejected result: `work/bench/knlskolznnxw_8688cc914f67`
  - specialized apply median: `522316 loops/s` (`-0.55%`)
  - verify pass: `298515 loops/s`
  - no-refcount diagnostic median: `643962 loops/s` (`-0.97%`)
  - latest summarized pystone code size: `58530 bytes`, `3496` machine blocks
- reason rejected: the direct hit counters and generated code size were
  unchanged, and the refcount-enabled median regressed. The attribute operand
  was not the hot-path cost worth chasing here.
- validation: `just fmt-rust soac_jit` passed; `cargo check -p soac_jit
  --tests` passed; `cargo test -p soac_jit field_index_specialized --
  --nocapture` passed; `just benchmark` produced the rejected result above.
  After reverting the experiment source, `just fmt-rust soac_jit` and
  `cargo check -p soac_jit --tests` passed.
- next baseline: `work/bench/knlskolznnxw_7d70b19fc78e`

## 2026-04-26 - Try typed inline bodies for direct method calls

- baseline: `work/bench/knlskolznnxw_f0adff97772e`
  - specialized apply median: `528248 loops/s`
  - verify pass: `295167 loops/s`
  - no-refcount diagnostic median: `654854 loops/s`
  - latest summarized pystone code size: `58424 bytes`, `3486` machine blocks
- observation: the refreshed deep profile still showed high `Proc1` entry and
  prologue samples, and pystone has hot direct method-call targets such as
  `Record.copy`. Ordinary profiled direct calls can already use the typed
  inline rewrite path, but direct method calls still lowered as guarded method
  calls followed by a direct edge.
- attempted change: let method direct-call plans request inline bodies, add a
  typed inline rewrite for `GuardedMethodCallTyped`, and emit the method guard
  as an owner type/version check on a stored receiver temp before jumping to the
  inlined target body.
- rejected result: `work/bench/knlskolznnxw_a24bd0918569`
  - specialized apply median: `472137 loops/s` (`-10.62%`)
  - verify pass: `277453 loops/s`
  - no-refcount diagnostic median: `558207 loops/s` (`-14.76%`)
  - latest summarized pystone code size: `58816 bytes`, `3499` machine blocks
  - counters: `runtime_decref=7961213`, `runtime_incref=6654036`,
    `call_direct=1616006`
- reason rejected: the inline method form made the generated pystone code
  larger and substantially slower in both refcount-enabled and no-refcount
  modes. The extra receiver temp/guard/fallback structure outweighed any direct
  edge entry savings.
- validation: before benchmarking, `just fmt-rust soac_ir_typed soac_opt
  soac_jit` passed; `cargo check -p soac_jit --tests` passed; `cargo test -p
  soac_opt direct_call_inline_candidate_requires_buildable_inline_body --
  --nocapture` passed; `cargo test -p soac_jit exact_list_item -- --nocapture`
  passed; `just pytest-fast
  tests/test_counter_dump_file.py::test_setitem_v3_profile_replay_records_hit_and_fallback_counters
  -q` passed; `cargo test -p soac_jit
  runtime_typed_v3_pipeline_emits_direct_calls_from_raw_profile_evidence --
  --nocapture` passed; `just benchmark` produced the rejected result above.
  After restoring the experiment source from the prior kept revision,
  `just fmt-rust soac_ir_typed soac_opt soac_jit` and `cargo check -p
  soac_jit --tests` passed.
- next baseline: `work/bench/knlskolznnxw_f0adff97772e`

## 2026-04-26 - Allow exact-string compare plans to read module constants

- baseline: `work/bench/knlskolznnxw_f0adff97772e`
  - specialized apply median: `528248 loops/s`
  - verify pass: `295167 loops/s`
  - no-refcount diagnostic median: `654854 loops/s`
  - latest summarized pystone code size: `58424 bytes`, `3486` machine blocks
- observation: the specialized render for `Func2` still emitted generic
  comparison for the materialized `CharLoc >= 'W'` test. The v3 exact-string
  return planner could handle local/local operands, but pystone's comparison is
  local/module-constant and mechanical region inputs only accepted named local
  PyObject inputs.
- kept change: add a `ModuleConstant` region input source for borrowed PyObject
  inputs, load it mechanically in JIT region input setup, and use that source
  for exact-string branch/return compare plans. This lets `Func2` `instr_id #14`
  select `PyObjectRichCompareBool` plus Python-bool materialization. `instr_id
  #19` still has no selected plan because the profile counter for that site is
  zero.
- kept result: `work/bench/knlskolznnxw_19ea3e660b14`
  - specialized apply median: `533953 loops/s` (`+1.08%`)
  - verify pass: `290070 loops/s`
  - no-refcount diagnostic median: `670618 loops/s` (`+2.41%`)
  - latest summarized pystone code size: `58752 bytes`, `3503` machine blocks
  - counters: `runtime_decref=8062213`, `runtime_incref=6957036`,
    `operator_hot_shapes=3333007`
- reason kept: the refcount-enabled median improved and the no-refcount
  diagnostic improved, with no specialization deopts or guard failures reported.
  The tradeoff is a small pystone code-size increase from the extra guarded
  exact-unicode path.
- validation: `just fmt-rust soac_ir_typed soac_opt soac_jit` passed; `cargo
  test -p soac_ir_typed prepares_region_module_constant_inputs -- --nocapture`
  passed; `cargo test -p soac_opt plans_exact_str_compare_return --
  --nocapture` passed; `cargo check -p soac_jit --tests` passed; specialized
  `render_instr_typed` showed the new `Func2` `instr_id #14` plan; `just
  benchmark` produced the kept result above.
- next baseline: `work/bench/knlskolznnxw_19ea3e660b14`

## 2026-04-26 - Try compact-int binary returns with module constants

- baseline: `work/bench/knlskolznnxw_19ea3e660b14`
  - specialized apply median: `533953 loops/s`
  - verify pass: `290070 loops/s` in the one-off run, `302599 loops/s` when
    refreshed by `benchmark-deep-profile-from-profile`
  - no-refcount diagnostic median: `670618 loops/s`
  - latest summarized pystone code size: `58752 bytes`, `3503` machine blocks
- observation: refreshed CLIF still showed generic `PyNumber_Add`,
  `PyNumber_Subtract`, and `PyNumber_Multiply` calls in hot pystone functions.
  Some v3 exact-int binary-return plans only accepted local/local operands, so
  local-plus-literal expressions such as `x + 1` could stay generic.
- attempted change: add a compact-int binary-return shape for one local/cell
  exact-int operand plus one `i64` module constant. The hot path unboxed the
  local operand, used an `I64` planned constant, emitted checked machine
  arithmetic, and materialized a Python long. The generic fallback materialized
  the constant and called the original Python numeric operation.
- rejected result: `work/bench/knlskolznnxw_b02120466e25`
  - specialized apply median: `535132 loops/s` (`+0.22%`)
  - verify pass: `300873 loops/s`
  - no-refcount diagnostic median: `692530 loops/s` (`+3.27%`)
  - latest summarized pystone code size: `60138 bytes`, `3612` machine blocks
  - counters: `operator_hot_shapes=2525007`, `runtime_decref=8062213`,
    `runtime_incref=6957036`
- reason rejected: the production median gain was too small for the generated
  code-size cost (`+1386` bytes and `+109` machine blocks versus the baseline).
  The stronger no-refcount diagnostic suggests refcount/materialization costs
  dominate this shape, so the broader constant-return plan is not the right
  production tradeoff yet.
- validation before rejection: `just fmt-rust soac_opt` passed; `cargo test -p
  soac_opt plans_compact_int_binary_return_with_module_constant_operand --
  --nocapture` passed; specialized `render_instr_typed` showed a new
  `CheckedI64Add` plan in `Proc0`; `cargo check -p soac_jit --tests` passed;
  `just benchmark` produced the rejected result above. The experiment was then
  removed.
- next baseline: `work/bench/knlskolznnxw_19ea3e660b14`

## 2026-04-26 - Exact-Unicode generic getitem helper fast path

- baseline: `work/bench/knlskolznnxw_19ea3e660b14`
  - specialized apply median: `533953 loops/s`
  - verify pass: `290070 loops/s` in the one-off run, `302599 loops/s` when
    refreshed by `benchmark-deep-profile-from-profile`
  - no-refcount diagnostic median: `670618 loops/s`
  - latest summarized pystone code size: `58752 bytes`, `3503` machine blocks
- observation: `Func2` still had two hot string subscript sites with
  `getitem_hot_shapes=101000` each and zero specialized hits. The perf profile
  also still showed `unicode_subscript`, so the existing generic
  `dp_jit_pyobject_getitem` wrapper looked like a narrow place to test an exact
  `str` plus exact compact `int` fast path before adding a new v3 item shape.
- attempted change: teach `dp_jit_pyobject_getitem` to recognize exact Unicode
  objects indexed by exact compact `int`, normalize negative indices, call the
  public Unicode C API directly, and fall back to `PyObject_GetItem` for every
  other object/key shape.
- rejected result: `work/bench/knlskolznnxw_7a33b9af1689`
  - specialized apply median: `526541 loops/s` (`-1.39%`)
  - verify pass: `300200 loops/s`
  - no-refcount diagnostic median: `645715 loops/s` (`-3.71%`)
  - latest summarized pystone code size: `58752 bytes`, `3503` machine blocks
- reason rejected: the helper-level fast path improved verify, but it regressed
  both production apply and the no-refcount diagnostic without changing
  generated pystone code size. The missed Unicode subscript shape is not worth
  hiding inside the generic getitem wrapper.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo test -p
  soac_jit pyobject_getitem_helper_fast_paths_exact_unicode_compact_int --
  --nocapture` passed; `cargo check -p soac_jit --tests` passed; `just
  benchmark` produced the rejected result above. The experiment was then
  removed.
- next baseline: `work/bench/knlskolznnxw_19ea3e660b14`

## 2026-04-26 - Direct `PyType_GenericAlloc` constructor allocation import

- baseline: `work/bench/knlskolznnxw_19ea3e660b14`
  - specialized apply median: `533953 loops/s`
  - verify pass: `290070 loops/s` in the one-off run, `302599 loops/s` when
    refreshed by `benchmark-deep-profile-from-profile`
  - no-refcount diagnostic median: `670618 loops/s`
  - latest summarized pystone code size: `58752 bytes`, `3503` machine blocks
- observation: `PyType_GenericAlloc` remained a visible runtime hotspot from
  `Record` construction/copying, while SOAC called it through the thin
  `dp_jit_pytype_generic_alloc` wrapper.
- attempted change: import `PyType_GenericAlloc` directly into generated JIT
  code, registering the real CPython C-API symbol through `dlsym` and keeping
  the existing `dp_jit_pytype_generic_alloc` wrapper as the fallback address if
  direct symbol resolution fails.
- rejected result: `work/bench/knlskolznnxw_2aaa58d9f393`
  - specialized apply median: `527572 loops/s` (`-1.19%`)
  - verify pass: `304898 loops/s`
  - no-refcount diagnostic median: `659971 loops/s` (`-1.59%`)
  - latest summarized pystone code size: `58752 bytes`, `3503` machine blocks
- reason rejected: verify improved, but both production apply and no-refcount
  diagnostics regressed with unchanged generated pystone code size. The wrapper
  was not the limiting cost for constructor allocation.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `just benchmark` produced the rejected result
  above. The experiment was then removed.
- next baseline: `work/bench/knlskolznnxw_19ea3e660b14`

## 2026-04-26 - Remove duplicate split-value insertion-order checks

- baseline: `work/bench/knlskolznnxw_19ea3e660b14`
  - specialized apply median: `533953 loops/s`
  - verify pass: `302599 loops/s` when refreshed by
    `benchmark-deep-profile-from-profile`
  - no-refcount diagnostic median: `670618 loops/s`
  - latest summarized pystone code size: `58752 bytes`, `3503` machine blocks
- observation: `soac_runtime_store_field_indexed_inline_values_trusted`
  remained a visible hotspot, and its insertion-order helper rechecked split
  values capacity and `u8` index bounds that the three store helpers had already
  checked immediately before mutation.
- attempted change: replace the checked insertion-order helper with an
  unchecked helper guarded by debug assertions, so first-insert field stores no
  longer repeat the same branch work after writing the split slot.
- rejected result: `work/bench/knlskolznnxw_0d7ad46ef9be`
  - specialized apply median: `521362 loops/s` (`-2.36%`)
  - verify pass: `302728 loops/s`
  - no-refcount diagnostic median: `644872 loops/s` (`-3.84%`)
  - latest summarized pystone code size: `58752 bytes`, `3503` machine blocks
- reason rejected: despite unchanged generated pystone code size and a flat
  verify pass, both production apply and no-refcount diagnostics regressed. The
  branch cleanup likely disturbed the runtime helper layout more than it removed
  useful work.
- validation before rejection: `cargo fmt --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo check --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `just benchmark` produced the
  rejected result above. The experiment was then removed.
- next baseline: `work/bench/knlskolznnxw_19ea3e660b14`

## 2026-04-26 - Let exact-string compare regions borrow indexed globals

- baseline: `work/bench/knlskolznnxw_19ea3e660b14`
  - specialized apply median: `533953 loops/s`
  - verify pass: `302599 loops/s` when refreshed by
    `benchmark-deep-profile-from-profile`
  - no-refcount diagnostic median: `670618 loops/s`
  - latest summarized pystone code size: `58752 bytes`, `3503` machine blocks
- observation: after module-constant exact-string compare support, pystone still
  had hot `str`/`str` comparison sites where one operand was a lowered module
  global such as `Char1Glob` or `Char2Glob`. Those sites stayed on
  `PyObject_RichCompare`, and the deep profile still showed rich-compare,
  Unicode compare, and TLS overhead.
- kept change: add an explicit v3 region input source for selected indexed
  global loads. Exact-string hot regions borrow directly from the guarded
  module-dict slot; their local fallback regions reload the global with the
  normal owned global-load path before running generic Python comparison.
- accepted result: `work/bench/knlskolznnxw_07c32271da7b`
  - specialized apply median: `541078 loops/s` (`+1.33%`)
  - verify pass: `303956 loops/s`
  - no-refcount diagnostic median: `682238 loops/s` (`+1.73%`)
  - latest summarized pystone code size: `59510 bytes`, `3553` machine blocks
  - counters: `operator_hot_shapes` dropped from `3333007` to `2828007`, with
    zero deopts and zero guard failures
- reason kept: the production apply median improved enough to justify the
  moderate code-size increase (`+758` bytes and `+50` machine blocks), and the
  no-refcount diagnostic moved in the same direction.
- validation: `just fmt-rust soac_ir_typed soac_opt soac_jit` passed; `cargo
  test -p soac_opt plans_exact_str_compare_branch_with_indexed_global_operand
  -- --nocapture` passed; `cargo check -p soac_jit --tests` passed; `just
  benchmark` produced the accepted result above.
- next baseline: `work/bench/knlskolznnxw_07c32271da7b`

## 2026-04-26 - Let exact-int operator regions borrow indexed globals

- baseline: `work/bench/knlskolznnxw_07c32271da7b`
  - specialized apply median: `541078 loops/s`
  - verify pass: `303956 loops/s` in the original one-off run, later refreshed
    to `305900 loops/s` by `benchmark-deep-profile-from-profile`
  - no-refcount diagnostic median: `682238 loops/s`
  - latest summarized pystone code size: `59510 bytes`, `3553` machine blocks
- observation: after exact-string indexed-global inputs, the deep profile still
  showed generic `PyNumber_Add` and `PyObject_RichCompare` calls in hot pystone
  functions. Several exact-int regions were still limited to local/cell
  operands even when profile data had already selected the module-global load.
- kept change: reuse the v3 indexed-global region input source for exact-int
  operator regions. Hot regions borrow directly from the guarded module-dict
  slot before compact-int guards and unboxing; fallback regions reload the
  global through the normal owned global-load path before generic Python
  operation lowering.
- accepted result: `work/bench/knlskolznnxw_a388a9db45fd`
  - specialized apply median: `559372 loops/s` (`+3.38%`)
  - verify pass: `323521 loops/s`
  - no-refcount diagnostic median: `674349 loops/s` (`-1.16%`)
  - latest summarized pystone code size: `63314 bytes`, `3795` machine blocks
  - counters: `operator_hot_shapes` dropped from `2828007` to `1414007`, with
    zero deopts and zero guard failures
- reason kept: production apply and verify both improved substantially despite
  the added code size (`+3804` bytes and `+242` machine blocks). The no-refcount
  diagnostic regressed slightly, but the production refcount-enabled path is
  the benchmark gate.
- validation: `just fmt-rust soac_opt` passed; `cargo test -p soac_opt
  compact_int -- --nocapture` passed; `cargo check -p soac_jit --tests` passed;
  `just benchmark` produced the accepted result above.
- next baseline: `work/bench/knlskolznnxw_a388a9db45fd`

## 2026-04-26 - Exact-float binary return regions

- baseline: `work/bench/knlskolznnxw_a388a9db45fd`
  - specialized apply median: `559372 loops/s`
  - verify pass: `323521 loops/s` in the original one-off run, later refreshed
    to `329301 loops/s` by `benchmark-deep-profile-from-profile`
  - no-refcount diagnostic median: `674349 loops/s`
  - latest summarized pystone code size: `63314 bytes`, `3795` machine blocks
- observation: the post-indexed-global deep profile still showed generic float
  arithmetic in hot pystone functions, including `PyNumber_Subtract`,
  `PyNumber_Multiply`, `PyNumber_TrueDivide`, `PyFloat_FromDouble`, and
  `PyLong_AsDouble`. The remaining `operator_hot_shapes` counters were mostly
  untagged exact-type shapes, because profile tagging only recognized ints and
  strings before this experiment.
- attempted change: add a `Float` exact-type profile tag, exact-float v3
  alternatives for `+`, `-`, `*`, and `/`, mechanical `f64` plan operations,
  guarded `PyFloat` unboxing, and `PyFloat_FromDouble` materialization.
- rejected result: `work/bench/knlskolznnxw_d1a28fbb885f`
  - specialized apply median: `552280 loops/s` (`-1.27%`)
  - verify pass: `319461 loops/s`
  - no-refcount diagnostic median: `717306 loops/s` (`+6.37%`)
  - latest summarized pystone code size: `63314 bytes`, `3795` machine blocks
  - counters: `operator_hot_shapes=1414007`, with zero deopts and zero guard
    failures
- reason rejected: the diagnostic no-refcount run improved, but the production
  refcount-enabled apply median regressed. This suggests the f64 fast path is
  not useful until float materialization/refcount pressure is reduced or the
  float value can stay scalar across more than a single return expression.
- validation before rejection: `just fmt-rust soac_ir_typed soac_opt soac_jit`
  passed; `cargo test -p soac_opt planner_v3 -- --nocapture` passed; `cargo
  test -p soac_opt alternatives_v3 -- --nocapture` passed; `cargo test -p
  soac_opt evidence_v3 -- --nocapture` passed; `cargo check -p soac_jit
  --tests` passed; `just benchmark` produced the rejected result above. The
  experiment was then removed.
- next baseline: `work/bench/knlskolznnxw_a388a9db45fd`

## 2026-04-26 - Lower sync for loops through next default sentinel

- baseline: `work/bench/knlskolznnxw_a388a9db45fd`
  - specialized apply median: `559372 loops/s`
  - verify pass: `323521 loops/s` in the original one-off run, later refreshed
    to `329301 loops/s` by `benchmark-deep-profile-from-profile`
  - no-refcount diagnostic median: `674349 loops/s`
  - latest summarized pystone code size: `63314 bytes`, `3795` machine blocks
- observation: the accepted range-next and StopIteration fast paths still left
  statement `for` loops lowered as exception-driven control flow. For pystone,
  each lowered sync loop could therefore allocate and match `StopIteration`
  at normal loop exhaustion even when the iterator target was the specialized
  exact range iterator.
- kept change: lower sync statement `for` loops to a fresh sentinel object plus
  `next(iterator, sentinel)` and an identity check, while leaving async `for`
  on the existing `StopAsyncIteration` path. Extend the existing
  `next(range_iterator)` vectorcall fast path so its generated helper accepts
  `nargs == 2` and returns the default value on exhaustion instead of raising
  `StopIteration`.
- accepted result: `work/bench/sentinel-for-loop-confirm/knlskolznnxw_aa20cc37925e`
  - specialized apply median: `574244 loops/s` (`+2.66%`)
  - verify pass: `329870 loops/s`
  - no-refcount diagnostic median: `763475 loops/s` (`+13.22%`)
  - latest summarized pystone code size: `59770 bytes`, `3565` machine blocks
  - counters: zero deopts and zero guard failures; `runtime_decref` increased
    from `8062213` to `8163218`, but production apply still improved
- first run note: `work/bench/knlskolznnxw_aa20cc37925e` had apply median
  `557064 loops/s` (`-0.41%`), no-refcount median `744848 loops/s`, and the
  same `59770` byte code-size summary. Because code size and diagnostic
  throughput moved strongly in the right direction, a confirmation run was
  used before deciding.
- reason kept: the confirmation production median beat the current accepted
  baseline and the generated pystone body shrank by `3544` bytes and `230`
  machine blocks. The larger no-refcount win suggests this also removes real
  control-flow/code-size overhead, even though refcount traffic still hides
  part of the gain.
- validation: `just fmt-rust soac_lowering soac_jit` passed; `cargo test -p
  soac_lowering lowers_for_else_break_into_basic_blocks -- --nocapture` passed;
  `cargo check -p soac_jit --tests` passed; `just pytest-fast
  tests/test_runtime_builtin_primitives.py -q` passed; `just benchmark`
  produced the accepted result above.
- next baseline:
  `work/bench/sentinel-for-loop-confirm/knlskolznnxw_aa20cc37925e`

## 2026-04-26 - Non-null refcount helpers for exact-list setitem

- baseline:
  `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: Proc8's perf-annotated VCode still showed hot refcount blocks in
  exact-list setitem. The replacement value is guarded non-null before the
  fast path, and CPython list item slots are non-null for valid in-bounds list
  entries, but the inlined generic refcount helpers still emitted null checks.
- attempted change: add `soac_runtime_incref_nonnull` and
  `soac_runtime_decref_nonnull` runtime helpers, make them inlineable runtime
  CLIF helpers, and call them from exact-list setitem fast paths only.
- rejected result: `work/bench/knlskolznnxw_7dc8c49c48f6`
  - specialized apply median: `577780 loops/s` (`-1.64%`)
  - verify pass: `333529 loops/s`
  - no-refcount diagnostic median: `760218 loops/s` (`-1.33%`)
  - latest summarized pystone code size: `60279 bytes`, `3609` machine blocks
- reason rejected: code size shrank by `32` bytes, but production apply and
  the no-refcount diagnostic both regressed. The helper split also changed the
  runtime refcount counter shape for this path, so the small code-size win did
  not justify keeping another refcount helper variant.
- validation before rejection: `cargo fmt --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `just fmt-rust soac_jit` passed;
  `cargo check --manifest-path crates/soac_jit_runtime/Cargo.toml` passed;
  `cargo check -p soac_jit --tests` passed; `cargo test -p soac_jit
  runtime_clif_refcount -- --nocapture` passed; `just benchmark` produced the
  rejected result above. The experiment was then removed.
- next baseline:
  `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Effect-only typed direct-call inlining

- baseline:
  `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: hot Proc0 still paid direct-call dispatch and recursive-call
  overhead for side-effect-only calls such as `Proc5()`, `Proc4()`, and
  `Proc8(...)`. The existing typed direct-call inliner only rewrites
  `target = f(...)` stores, so effect-only call statements were never inline
  candidates.
- attempted change: allow v3 to mark effect-only direct calls as inline
  candidates, then have the typed inliner allocate a synthetic result local,
  run the existing guarded inline/fallback shape, and delete the discarded
  result in the shared cleanup block.
- rejected result: `work/bench/knlskolznnxw_de36e2e870ef`
  - specialized apply median: `527203 loops/s` (`-10.25%`)
  - verify pass: `335816 loops/s`
  - no-refcount diagnostic median: `683738 loops/s` (`-11.26%`)
  - latest summarized pystone code size: `62903 bytes`, `3698` machine blocks
  - counters: `getitem_specialized` fell from `808002` to `2`, and
    `setitem_specialized` fell from `707002` to `2`
- reason rejected: the inliner attached, but it moved hot bodies such as
  `Proc8` into the caller after per-function plan selection, so their original
  exact-list and item-store specializations no longer matched the inlined
  function context. Any future effect-only inlining needs to preserve or remap
  the callee's per-function specialization plans before it can be a win.
- validation before rejection: `just fmt-rust soac_opt` passed; `cargo check -p
  soac_jit --tests` passed; `cargo test -p soac_jit inline_direct --
  --nocapture` passed; `cargo test -p soac_opt direct_call_inline --
  --nocapture` passed; `just benchmark` produced the rejected result above. The
  experiment was then removed.
- next baseline:
  `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Inline trusted indexed-field store helper

- baseline:
  `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - specialized apply mean: `592714 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: deep-profile symbol self time still showed
  `soac_runtime_store_field_indexed_inline_values_trusted` at about `5.58%`,
  and the helper was not selected by the normal runtime-support inliner because
  its generated CLIF has about `196` instructions, above the `128` instruction
  cap.
- attempted change: keep the global runtime-support inline cap unchanged, but
  allow only the trusted indexed-field store helper to inline up to `256`
  instructions.
- rejected result: `work/bench/knlskolznnxw_a3eba02df5d6`
  - specialized apply median: `592999 loops/s` (`+0.95%`)
  - specialized apply mean: `592814 loops/s` (`+0.02%`)
  - verify pass: `326891 loops/s`
  - no-refcount diagnostic median: `735705 loops/s` (`-4.51%`)
  - latest summarized pystone code size: `70018 bytes`, `4279` machine blocks
- reason rejected: the production median improved, but the mean was flat against
  the baseline, no-refcount regressed sharply, and generated pystone code grew
  by `9707` bytes and `670` machine blocks. The helper is too large to inline
  broadly without a more selective call-site policy.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `cargo test -p soac_jit runtime_clif --
  --nocapture` passed; `just benchmark` produced the rejected result above. The
  experiment was then removed.
- next baseline:
  `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Structured exact-unicode getitem specialization

- baseline: `work/bench/knlskolznnxw_e80c7a557ade`
  - specialized apply median: `587402 loops/s`
  - specialized apply mean: `592714 loops/s`
  - verify pass: `336972 loops/s`
  - no-refcount diagnostic median: `770463 loops/s`
  - latest summarized pystone code size: `60311 bytes`, `3609` machine blocks
- observation: `Func2` still called generic `dp_jit_pyobject_getitem` for
  `StrParI1[IntLoc]` and `StrParI2[IntLoc + 1]`. The profile recorded shape
  `0` at those sites because item-shape evidence only recognized
  exact-list/exact-int.
- attempted change: add a distinct exact-unicode/exact-int item shape to the v3
  item-access plan, select it only for getitem, and emit a guarded compact-ASCII
  fast path that materialized the selected character through the existing
  `soac_runtime_builtin_chr_i64` helper.
- rejected result: `work/bench/knlskolznnxw_54a55d5cd9e7`
  - specialized apply median: `583628 loops/s` (`-0.64%`)
  - specialized apply mean: `590866 loops/s` (`-0.31%`)
  - verify pass: `332902 loops/s`
  - no-refcount diagnostic median: `774683 loops/s` (`+0.55%`)
  - latest summarized pystone code size: `61151 bytes`, `3663` machine blocks
  - counters: `getitem_specialized` rose from `808002` to `1010002`
- reason rejected: the new shape did attach and removed the remaining generic
  getitem sites in `Func2`, but the production refcount-enabled apply median and
  mean both regressed while generated pystone code grew by `840` bytes and `54`
  machine blocks. The no-refcount diagnostic alone was not enough to keep it.
- validation before rejection: `just fmt-rust soac_ir_typed soac_opt soac_jit`
  passed; `cargo check -p soac_jit --tests` passed; `cargo test -p
  soac_ir_typed exact_list_item -- --nocapture` passed; `cargo test -p soac_opt
  exact_list_item -- --nocapture` passed; `just benchmark` produced the rejected
  result above.
- next baseline: `work/bench/knlskolznnxw_e80c7a557ade`

## 2026-04-26 - Static `iter(x)` runtime primitive

- baseline:
  `work/bench/sentinel-for-loop-confirm/knlskolznnxw_aa20cc37925e`
  - specialized apply median: `574244 loops/s`
  - verify pass: `329870 loops/s`
  - no-refcount diagnostic median: `763475 loops/s`
  - latest summarized pystone code size: `59770 bytes`, `3565` machine blocks
- observation: the accepted sentinel loop lowering still left compiler-created
  one-argument `iter(range(...))` setup on the generic vectorcall path. A
  previous `iter` vectorcall hook was rejected because it added a branch to all
  vectorcalls; this attempt moves statically resolved `iter(x)` calls onto the
  existing runtime-primitive descriptor path instead.
- kept change: add a direct `soac_runtime_builtin_iter_object` primitive for
  static one-argument `iter(x)` calls. The helper preserves CPython semantics by
  calling `PyObject_GetIter(x)` and returning the owned iterator result.
- first result: `work/bench/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `587076 loops/s` (`+2.23%`)
  - verify pass: `335827 loops/s`
  - no-refcount diagnostic median: `760511 loops/s` (`-0.39%`)
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
  - counters: `call_hot_targets` dropped from `2222189` to `2121184`; deopts
    and guard failures stayed at zero
- confirmation result:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`
  - specialized apply median: `582486 loops/s` (`+1.44%`)
  - verify pass: `332080 loops/s`
  - no-refcount diagnostic median: `766196 loops/s` (`+0.36%`)
  - latest summarized pystone code size: `59812 bytes`, `3586` machine blocks
- reason kept: the confirmation production median stayed above the previous
  baseline, the no-refcount diagnostic recovered to a small win, and the
  counter drop shows the generated code avoided about one benchmark pass worth
  of generic call-target sampling for `iter`.
- validation: `just fmt-rust soac_jit` passed; `cargo fmt --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo check --manifest-path
  crates/soac_jit_runtime/Cargo.toml` passed; `cargo check -p soac_jit --tests`
  passed; `cargo test -p soac_jit direct_abi -- --nocapture` passed; `cargo
  test -p soac_jit runtime_clif_builtin_primitive_symbols_are_available --
  --nocapture` passed; `just pytest-fast tests/test_runtime_builtin_primitives.py
  -q` passed; both benchmark runs produced the kept result above.
- next baseline:
  `work/bench/iter-runtime-primitive-confirm/knlskolznnxw_1415b753ad5d`

## 2026-04-26 - Exact-int materialization through `PyLong_FromLong`

- baseline:
  `work/bench/sentinel-for-loop-confirm/knlskolznnxw_aa20cc37925e`
  - specialized apply median: `574244 loops/s`
  - verify pass: `329870 loops/s`
  - no-refcount diagnostic median: `763475 loops/s`
  - latest summarized pystone code size: `59770 bytes`, `3565` machine blocks
- observation: deep profiling still attributed about `1.21%` of samples to
  `PyLong_FromLongLong`, while this Linux 64-bit target can represent the same
  exact-int materialization range through CPython's `PyLong_FromLong` ABI.
- attempted change: switch the exact-int materialization import used by typed
  JIT codegen from `PyLong_FromLongLong` to `PyLong_FromLong`, with the matching
  registered CPython wrapper for helper-frame mode.
- first result: `work/bench/knlskolznnxw_0db14c27c921`
  - specialized apply median: `582632 loops/s` (`+1.46%`)
  - verify pass: `328150 loops/s`
  - no-refcount diagnostic median: `739979 loops/s` (`-3.08%`)
  - latest summarized pystone code size: `59770 bytes`, `3565` machine blocks
- confirmation result:
  `work/bench/pylong-from-long-confirm/knlskolznnxw_0db14c27c921`
  - specialized apply median: `570313 loops/s` (`-0.68%`)
  - verify pass: `334264 loops/s`
  - no-refcount diagnostic median: `752291 loops/s` (`-1.46%`)
- reason rejected: the first production median improved, but the confirmation
  run fell below the baseline and both no-refcount diagnostics regressed. The
  generated code size and specialization counters were unchanged, so there was
  no structural signal to justify keeping a noisy helper-call substitution.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo check -p
  soac_jit --tests` passed; `cargo test -p soac_jit
  specialized_jit_opt_v3_exact_int -- --nocapture` passed; both `just
  benchmark` runs produced the rejected results above. The experiment was then
  removed.
- next baseline:
  `work/bench/sentinel-for-loop-confirm/knlskolznnxw_aa20cc37925e`

## 2026-04-26 - Two-argument `next` default runtime primitive

- baseline:
  `work/bench/sentinel-for-loop-confirm/knlskolznnxw_aa20cc37925e`
  - specialized apply median: `574244 loops/s`
  - verify pass: `329870 loops/s`
  - no-refcount diagnostic median: `763475 loops/s`
  - latest summarized pystone code size: `59770 bytes`, `3565` machine blocks
- observation: after lowering sync `for` loops to `next(iterator, sentinel)`,
  deep profiling still showed residual `py_vectorcall_hook` and range iterator
  dispatch around the compiler-generated `next(..., default)` calls.
- attempted change: add a typed runtime primitive for static two-argument
  `next(iterator, default)` calls. The helper kept an exact CPython
  `range_iterator` fast path and used `PyIter_Next` plus an owned default
  return for generic iterators.
- rejected result: `work/bench/knlskolznnxw_986aa267c6e2`
  - specialized apply median: `573020 loops/s` (`-0.21%`)
  - verify pass: `322625 loops/s`
  - no-refcount diagnostic median: `754944 loops/s` (`-1.12%`)
  - latest summarized pystone code size: `59821 bytes`, `3587` machine blocks
  - counters: `call_hot_targets` dropped from `2222189` to `1717133`, but
    deopts and guard failures stayed at zero
- reason rejected: the primitive removed countered call-target evidence, but
  production apply and the no-refcount diagnostic both regressed and generated
  pystone code grew by `51` bytes and `22` machine blocks. The direct helper
  did not beat the existing vectorcall fast path.
- validation before rejection: `just fmt-rust soac_jit` passed; `cargo fmt
  --manifest-path crates/soac_jit_runtime/Cargo.toml` passed; `cargo check
  --manifest-path crates/soac_jit_runtime/Cargo.toml` passed; `cargo test -p
  soac_jit direct_abi -- --nocapture` passed; `cargo check -p soac_jit --tests`
  passed; `just pytest-fast tests/test_runtime_builtin_primitives.py -q`
  passed; `just benchmark` produced the rejected result above. The experiment
  was then removed, and `just fmt-rust soac_jit`, `cargo fmt --manifest-path
  crates/soac_jit_runtime/Cargo.toml`, `cargo check -p soac_jit --tests`, and
  `cargo check --manifest-path crates/soac_jit_runtime/Cargo.toml` passed.
- next baseline:
  `work/bench/sentinel-for-loop-confirm/knlskolznnxw_aa20cc37925e`
