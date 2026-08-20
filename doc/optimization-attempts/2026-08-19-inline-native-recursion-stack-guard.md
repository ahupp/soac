---
title: "Inline Native Recursion Stack Guard"
---

# Inline native recursion stack guard

- Status: **IN PROGRESS; bounded production strategy selected; actual
  unchanged-production stock/Profile → Verify → Apply transformed
  compatibility GREEN 1 passed / 1.59 seconds; genuine actual
  production-used Cranelift structured optimization RED → first
  narrower-guard GREEN 1 passed / 574 filtered → final package-formatted
  maximum-margin structured GREEN; frozen actual candidate
  stock/Profile → Verify → Apply transformed integration GREEN 1 passed /
  1.63 seconds; full JIT 575 / optimizer 214 / typed IR 54, broad
  transformed 17 / 17, combined test-target and scoped format checks
  GREEN; root-owned release smoke GREEN 8 / 8 with identical source
  bodies and hidden trampoline growth 36,500 → 38,692 bytes; first
  normally sampled fixed-eight candidate NEGATIVE / INCONCLUSIVE,
  official previous SOAC 0.9700065211876199x with paired deltablue
  regression; definitive three-round chaos paired 0.975258x
  [0.957298, 0.992206] REGRESSION; FIRST ITERATION REJECTED AS-IS;
  lossless first-candidate deltablue profile confirms exact-trampoline
  helper elimination despite net harm; independently source-proved
  three-branch-elision refinement package-formatted and genuine
  actual-production structured RED → GREEN, hot conditional branches
  **4 → 1**; frozen actual refined stock/Profile → Verify → Apply
  transformed integration GREEN 1 passed / 1.60 seconds; fresh refined
  JIT 575 / optimizer 214 / typed IR 54, transformed 17 / 17 in 40.06
  seconds, combined test-target and scoped format checks GREEN; refined
  release smoke GREEN 8 / 8 with invariant source bodies and hidden
  trampoline bytes retained 36,500 → rejected first 38,692 → refined
  38,108; refined normal fixed-eight official stock
  0.6694448241941483x / previous SOAC 1.0016222298324013x; definitive
  targeted stock 0.525149227454957x / previous 1.0374660673409746x,
  repeated deltablue raw 1.064924x / paired 1.073872x and richards raw
  1.084732x / paired 1.072466x, chaos/comprehensions NEUTRAL; lossless
  deltablue / matched richards confirm reduced strict trampoline
  overhead; authoritative full gate GREEN 1,235 transformed nodeids /
  98 isolated batches / 8 workers / 98 passed; FULLY VALIDATED / RETAIN
  LANDING CANDIDATE, not yet landed**.
- Pacific date: **2026-08-19 PDT**.
- Integrated baseline: retained `main` change **`wrzzyrtx`**, commit
  **`c93baaf2`**.
- Candidate change: **`yknrqtlm`**, initially observed at mutable working
  commit **`9f6cf397`**; subsequent working-copy snapshots can change the
  commit ID.
- Outcome: determine whether existing shared exact-positional and generic
  vectorcall trampolines can use a
  conservative native-frame / live-thread-state stack guard to skip only
  demonstrably unnecessary CPython recursion-helper calls. There is **no
  claimed existing user-visible CPython behavior mismatch**.

## Hypothesis and evidence

- General-purpose opportunity: ordinary hot Python calls already enter
  through shared exact-positional or generic vectorcall trampolines that
  obtain the live thread state. Every such trampoline originally
  unconditionally calls
  `dp_jit_enter_recursive_call`, whose private `enter_recursive_call_hook`
  ignores that existing thread-state argument and invokes
  `Py_EnterRecursiveCall`. The public CPython entrypoint reacquires the
  same live state through thread-local storage before testing the native
  C-stack interval. Most ordinary calls are far from that interval.
- The current retained lossless `richards` profile contains **244
  samples**. Its disjoint recursion-helper ancestry totals **2.459410%**,
  all inside exact trampolines: arity-one **1.639606%** and arity-three
  **0.819803%**. These are measured ancestry partitions, not a guaranteed
  end-to-end speedup.
- An available earlier `deltablue` profile contains **246 samples** and
  **8.129496%** recursion ancestry. Exact trampolines account for
  **7.316147 percentage points**, comprising arity-one **5.689448%** and
  arity-three **1.626699%**; other direct recursion handling accounts for
  **0.813349%** and is outside this bounded optimization. This earlier
  profile is supporting source/hotspot evidence, not a matched fresh
  profile of the current retained revision.
- Available `comprehensions` recursion ancestry is only **0.684554%**,
  with exact trampolines contributing **0.342277%**. `chaos` and
  `comprehensions` are controls; no improvement is assumed from small or
  unrelated recursion shares.
- The actual production call shape in
  `crates/soac_jit/src/jit/vectorcall.rs` obtains one live
  `thread_state_val` before the retained baseline unconditionally imports
  and calls the existing recursion helper. This production emitter is
  shared by **both exact-positional and generic vectorcall shapes**; the
  sampled target recursion cost happens to be in exact trampolines.
  `crates/soac_jit/src/config.rs` already sets Cranelift
  `preserve_frame_pointers=true`, so the generated native trampoline can
  request its actual frame pointer directly.
- Pinned `vendor/cpython/Include/internal/pycore_ceval.h` defines
  `_Py_MakeRecCheck` for the downward-growing target stack as the exact
  release-build native-address interval
  **`[soft - 32768, soft)`**. Pinned `pycore_pythonrun.h` makes the normal
  64-bit release stack margin **16,384 bytes**, while CPython debug,
  ASAN, and TSAN configurations use the documented maximum margin of
  **32,768 bytes** and therefore a native check interval of
  **`[soft - 65536, soft)`**. The specifically verified current
  `vendor/cpython/pyconfig` target is **64-bit aarch64 release,
  non-debug, non-ASAN, and non-TSAN**. The refined production guard uses
  the maximum **32,768-byte** margin unconditionally, conservatively
  covering both release and larger sanitizer/debug stack-margin values
  without requiring `build.rs` to reject or detect sanitizer builds.
  Private CPython layout assumptions still apply only to the pinned
  supported configuration; wider margin coverage is not a claim of
  arbitrary ABI portability.
- Pinned `vendor/cpython/Include/internal/pycore_tstate.h` places
  `_PyInterpreterFrame base_frame`, a reference count, `c_stack_top`,
  and then `c_stack_soft_limit` consecutively inside `_PyThreadStateImpl`.
  The public `PyThreadState.base_frame` field is at offset **80** and
  points to that embedded frame. For this validated 64-bit pinned
  layout, `_PyInterpreterFrame` is **88 bytes**; its following
  **8-byte** reference count and **8-byte** stack-top slot locate the
  current soft limit at **`base_frame + 104`**. Read the live thread state
  already acquired for this call; never cache it across calls or threads.
- Pinned `vendor/cpython/Lib/test/test_call.py`,
  `TestRecursion.test_margin_is_sufficient`, explicitly measures native
  vectorcall stack descent, adds a **25% safety allowance**, and verifies
  the result remains smaller than CPython's native stack margin. This
  supports widening the SOAC guard before delegating to the unchanged
  public recursion checker; it does not justify removing that checker
  near a boundary.
- A superficially larger `comprehensions` alternative has **9.246%**
  inclusive ancestry, but no sound bounded implementation within five or
  fewer production paths was identified; inclusive ancestry alone is not
  removable work. Inherited-method direct lookup would require roughly
  **five to seven production files** plus sound instance-shadowing and
  owner/type invalidation guards. Both alternatives are rejected for this
  bounded attempt, not claimed impossible in future work.
- Full-suite stock **1.10x** remains unmet. The complete pyperformance
  suite has not been measured; neither a small target subset nor a native
  hotspot establishes full-suite acceptance.

## Implementation and compatibility

- Saved, independently host-source-reviewed, package-formatted, and
  **FROZEN** production scope is **exactly two existing runtime files**:
  `crates/soac_jit/src/jit/runtime_context.rs` and
  `crates/soac_jit/src/jit/vectorcall.rs`. A focused structured assertion
  may use the existing `#[cfg(test)]`-only JIT test harness, and a new
  transformed integration belongs under `tests/`; neither is a third
  production path.
- Preserve the current first live thread-state lookup exactly. Use the
  existing pinned thread-state prefix / embedded-base-frame layout to
  read the **current** `c_stack_soft_limit`; obtain the real Cranelift
  `get_frame_pointer` value in the existing generated vectorcall emitter
  shared by exact-positional and generic shapes.
- Treat the widened downward-stack danger band as
  **`[soft - 65536, soft + 32768)`**. Compute its membership with the
  explicit wrapping unsigned predicate
  **`(frame_pointer - soft + 65536) <u 98304`**. Only when the current
  native frame is proven **outside** this entire widened band may the hot
  path skip the recursion helper and proceed directly.
- Using CPython's maximum documented **32,768-byte** native margin
  conservatively includes both the actual pinned release margin
  **16,384** and debug/ASAN/TSAN margin **32,768**. Extra release-build
  addresses in the widened cold band call the unchanged authoritative
  public helper, which returns **0** where no recursion check is needed.
  Widening the original interval added no hot-path branches or
  instructions compared with the initial narrower candidate, and its
  AArch64 immediates are encodable. The subsequent independently proved
  refinement removes **three** redundant presence branches, leaving one
  interval branch; actual throughput and generated-size effects still
  require measurement.
- The generated fast path is enabled only when the native pointer type is
  **64-bit** and the build target is **`aarch64` or `x86_64`**; all
  other architectures retain the existing unconditional helper. The
  rejected first iteration explicitly branched on a null current thread
  state, null embedded base frame, or zero soft limit. The refined
  production path removes those three branches because pinned CPython
  attaches a nonnull live state only after initializing its nonnull
  embedded frame and nonzero stack protection; existing direct callees
  already depend on the same invariants. Its single hot unsigned
  interval branch sends an in-band native frame to the **unchanged
  existing** `dp_jit_enter_recursive_call` → `Py_EnterRecursiveCall`
  cold path. That path reacquires thread-local state and owns exact
  CPython stack checks, error construction, exception raising points,
  wrong-fiber / wrong-stack behavior, and failure returns. No direct
  replacement for `_Py_CheckRecursiveCall` is introduced.
- The pinned initial soft-limit sentinel **`UINTPTR_MAX`** can skip the
  public helper when wrapping arithmetic proves the current frame outside
  the widened band. This is semantically equivalent because the pinned
  original public helper's `_Py_MakeRecCheck` would also return **0** for
  that state. Do **not** claim every uninitialized soft-limit value
  enters the cold path.
- No runtime mechanism detects an arbitrary CPython ABI/layout mismatch.
  Compatibility depends on the supported 64-bit configuration, private
  `#[repr(C)]` layout mirrors, pinned offsets, and real layout regression
  tests. The actual verified ABI is the pinned **aarch64** build;
  `x86_64` is accepted by the architecture condition but is not
  independently portability-validated here. Unsupported architectures
  retain the original helper. The maximum-margin guard covers documented
  release/debug/sanitizer **margin sizes**, but does not dynamically
  detect an arbitrary unrecognized ABI/layout change.
- The widened upper margin deliberately accounts for the difference
  between the generated frame pointer and the deeper native stack
  pointer eventually observed inside CPython's helper. Wrapping unsigned
  arithmetic avoids pointer-subtraction overflow assumptions and mirrors
  the pinned CPython address-as-integer convention. Unsupported pointer
  widths and architectures retain the original unconditional helper; the
  supported pinned layout is asserted by private mirrors and tests.
- Preserve Python recursion-limit semantics, `RecursionError` type and
  contextual message, exact exception propagation, argument evaluation,
  callbacks, finalizers, GIL/thread identity, context switching,
  concurrent threads, recursive kwargs / mixed Python-C calls, and
  existing generic vectorcall fallback. No thread-state cache, mutable
  global, exported helper, public API, public typed-IR operation,
  compiler-visible function body, or source-native body change is
  permitted.
- The optimization changes only **hidden exact-positional and generic
  vectorcall trampolines**;
  source direct-function-body native bytes / blocks and typed-IR coverage
  should remain invariant. Hidden trampoline bytes may increase and must
  be recorded explicitly; a material increase requires investigation.
  Existing non-trampoline direct recursion-helper callers remain
  unchanged.
- A genuine unchanged-semantics **production-used structured optimization
  RED** is established using the actual Cranelift trampoline builder. At
  the RED checkpoint, private
  **`emit_vectorcall_native_recursion_guard`** is extracted directly
  from the real shared exact/generic vectorcall trampoline while retaining
  the identical original unconditional public recursion-helper call.
  Private `#[repr(C)]` pinned-CPython layout mirrors and real `offset_of!`
  checks first pass: live thread-state `base_frame` offset **80**,
  embedded interpreter-frame size **88**, and frame-relative soft-limit
  offset **104** on the actual 64-bit target.
- The focused existing `#[cfg(test)]` JIT test builds a genuine Cranelift
  **`ir::Function` / `FunctionBuilder`** with the real imported helper and
  calls that exact production emitter. Its sole intended failing assertion
  is **`production trampoline guard must read its native frame pointer`**:
  actual `GetFramePointer` count **0**, expected **1**, after all pinned
  layout controls pass. This is real structured production CFG evidence,
  not rendered-IR text, a fabricated production stub, or a CPython-visible
  behavior failure.
- The focused RED required **25.83 seconds of compilation**. That is
  workflow/build setup overhead only, not workload throughput, test
  execution time, or optimization performance evidence.
- The same genuine actual production-emitter regression first turned
  **RED → GREEN: 1 passed / 574 filtered** for the historical initial
  **49,152-byte** interval. That real generated Cranelift CFG verified
  live-thread-state / base-frame offsets **80 / 104**, embedded-frame
  size **88**, exactly **two trusted soft-limit path loads**, one native
  frame-pointer read, and exactly **one** unchanged public recursion-helper
  call in the **COLD** block with **zero** helper calls on the **HOT**
  branch. This first GREEN is preserved as chronological evidence, not a
  test result for the final widened candidate.
- Before the first post-implementation transformed check, architect and
  independent reviewer accepted the safer maximum-margin refinement to
  the **98,304-byte** interval. The final package-formatted real
  production-emitter structured regression is now independently
  **GREEN**, proving the exact wrapping unsigned predicate
  **`(frame_pointer - soft + 65536) <u 98304`**, pinned layout offsets
  **80 / 104**, embedded-frame size **88**, and exactly **one COLD /
  zero HOT** original public recursion-helper calls. The frozen actual
  Profile → Verify → Apply candidate integration is independently
  **GREEN: 1 passed in 1.63 seconds** for the historical first candidate.
  Its subsequent one-branch refinement independently passes in **1.60
  seconds**, with fresh full libraries and broad transformed coverage
  GREEN. Refined measured performance, generated hidden-trampoline size,
  and full-gate success are not claimed.
- The frozen new transformed integration
  **`tests/test_native_recursion_stack_guard.py`** is independently
  **GREEN on unchanged production: 1 passed in 1.59 seconds**
  (**1.794 seconds outer pytest**). It compares actual stock against
  transformed **Profile → Verify → Apply**, including exact arities
  **0 / 1 / 2**, generic/default/keyword/variadic call shapes, finite
  recursion, safely Python-bounded `RecursionError` at limit **96**,
  recovery and changed limit **128**, propagated exceptions, and
  `ctypes` C↔Python callbacks.
- The same unchanged-production integration simultaneously verifies the
  main thread plus **two live worker threads**, distinct/current thread
  states and native stack pointers, `sys.setprofile`, owned-argument
  finalizers and exception cleanup, the pinned
  `_testinternalcapi` stack margin **16,384**, safe stack-protection
  set/reset, at least **40** genuine hot counter values, and actual
  source-native direct bodies. This is a **baseline compatibility GREEN,
  not an optimization RED or an existing CPython behavior bug**.
- An initial fixture-only reference-count mismatch came from retained
  Python traceback references; scoped exception cleanup corrected the
  fixture. It was **not** a production failure or a CPython semantics
  mismatch. The genuine production-CFG optimization RED turned GREEN on
  both the historical initial narrower guard and the final maximum-margin
  candidate.
- The frozen actual final maximum-margin candidate integration
  **`just pytest-fast tests/test_native_recursion_stack_guard.py -q`**
  now independently passes **1 test in 1.63 seconds**. Real stock plus
  transformed Profile → Verify → Apply preserve exact and generic /
  default / keyword / variadic calls, simultaneous main and two-worker
  live thread states, `ctypes` Python↔C callbacks, bounded recursion and
  recovery, exceptions, finalizers, profiling, actual native direct
  bodies, and recorded counters. The package had already been formatted
  before pytest; a one-time **23.92-second debug-extension build** is
  workflow-only setup, not test runtime or throughput evidence.
- The first, subsequently rejected implementation independently passes
  full Rust libraries **575 / 214 / 54**, broad transformed coverage
  **17 / 17 in 40.41 seconds**, and combined / scoped checks. Its release
  smoke passes all eight workloads, but normal and clean repeated
  comparisons establish a genuine `chaos` regression; its lossless
  causal profile confirms helper elimination without establishing a net
  gain. The subsequent **one-branch refinement** freshly passes JIT
  **575 / 575**, optimizer **214 / 214**, typed IR **54 / 54**, broad
  transformed coverage **17 / 17 in 40.06 seconds**, combined
  optimizer/JIT `cargo check --tests`, and package-scoped formatting /
  format-check. Its release smoke independently passes all **8 actual
  Apply workers**, preserves every ordinary source body, and reduces
  hidden trampoline bytes from the rejected first iteration's **38,692**
  to **38,108**, still above retained **36,500**. Refined normally
  sampled fixed-eight comparison completes with official stock
  **0.6694448241941483x** and previous SOAC
  **1.0016222298324013x**. Definitive clean repeated targeted comparison
  independently confirms stock **0.525149227454957x** / previous SOAC
  **1.0374660673409746x**, significant raw and paired deltablue /
  richards gains, and neutral chaos / comprehensions; lossless delta and
  matched richards profiles confirm reduced trampoline self overhead.
  Status is **FULLY VALIDATED / RETAIN LANDING CANDIDATE**: authoritative
  `just test-all` independently passes **1,235 transformed pytest
  nodeids / 98 isolated batches / 8 workers / 98 passed / zero failed**,
  together with all Rust libraries. Landing is not yet complete, and the
  full-suite stock performance goal remains unmet.

## Benchmark protocol and coverage

- Fixed smoke / normally sampled selection:
  **`chaos,comprehensions,deltablue,fannkuch,float,nbody,richards,spectral_norm`**.
- Fixed repeated targeted selection:
  **`chaos,comprehensions,deltablue,richards`**; `deltablue` is the
  primary source-backed target, `richards` is secondary, and the other
  workloads are guardrails.
- Use repo-native `just pyperformance-compare` against the mode-matched
  retained previous-SOAC result. A release debug-single smoke only proves
  completion and generated coverage; its timings are not headline
  performance evidence. Run a normally sampled fixed-eight comparison and
  at least **three independently started, order-alternated targeted
  stock/SOAC rounds** before any retained improvement claim.
- Immediate retained release smoke is
  **`work/pyperformance/comparison-20260819-185033-swtmUh`**. Actual
  Apply source-native code totals **2,238,412 bytes / 147,769 blocks**;
  hidden vectorcall trampoline bytes total **36,500**. It contains
  **397 total JIT source rows, including adapters / 204 actual
  direct-function-body rows**, with **2,866 typed blocks / 204
  functions** across eight transformed benchmark workers.
- Candidate release debug-single smoke
  **`work/pyperformance/comparison-20260819-194520-S0YXF7`**, against
  mode-matched retained **185033**, completes all **eight actual measured
  Apply worker PIDs**. Every **397 total JIT source rows / 204 direct
  bodies** has identical source identity, native bytes, and machine
  blocks. Source-native totals remain exactly **2,238,412 bytes /
  147,769 blocks**, and typed coverage remains **2,866 blocks / 204
  functions**. All **3,816 structured events** contain **zero ERROR or
  CRITICAL** records.
- Only hidden `jitdump` trampoline code changes:
  **36,500 → 38,692 bytes (+2,192 bytes / +6.00548%)**. Exact
  positional trampoline arities **0 / 1 / 2 / 3 / 4 / 5 / 6** grow
  respectively **720 → 784**, **952 → 1,052**, **1,124 → 1,188**,
  **1,292 → 1,360**, **1,468 → 1,532**, **1,676 → 1,704**, and
  **1,856 → 1,920 bytes**. Per-workload hidden totals change `chaos`
  **6,692 → 7,052**, `comprehensions` **4,088 → 4,384**, `deltablue`
  **6,692 → 7,052**, `fannkuch` **952 → 1,052**, `float`
  **2,076 → 2,240**, `nbody` **6,512 → 6,836**, `richards`
  **7,412 → 7,836**, and `spectral_norm` **2,076 → 2,240 bytes**.
  Cold debug-single timings are **INVALID performance evidence**; smoke
  proves release coverage and hidden-size cost, not a speedup or
  regression.
- Refined one-branch release debug-single smoke
  **`work/pyperformance/comparison-20260819-201043-WFkwCD`**, against
  both mode-matched retained **185033** and rejected first candidate
  **194520**, independently passes all **8 actual measured Apply PIDs**.
  Every **397 total JIT source rows, including adapters / 204 actual
  direct-function bodies** retains exact source identities, native bytes,
  and blocks: **2,238,412 bytes / 147,769 blocks**. Typed coverage stays
  **2,866 blocks / 204 functions**, with the same **198 function
  identities**; all **3,816 structured events** are INFO and **zero** are
  errors. Hidden trampolines change retained **36,500 → rejected first
  38,692 → refined 38,108 bytes**: refined **+1,608 bytes / +4.4055%**
  against retained, but **−584 bytes / −1.5094%** against the rejected
  four-branch candidate. Exact arity **0 / 1 / 2 / 3 / 4 / 5 / 6**
  retained / first / refined sizes are respectively **720 / 784 / 756**,
  **952 / 1,052 / 1,024**, **1,124 / 1,188 / 1,200**,
  **1,292 / 1,360 / 1,328**, **1,468 / 1,532 / 1,504**,
  **1,676 / 1,704 / 1,672**, and **1,856 / 1,920 / 1,892 bytes**. Arity
  two grows **12 bytes** versus the rejected first implementation; arity
  five becomes **4 bytes smaller than retained**. Debug-single cold
  timings remain **INVALID performance evidence**. The refined normal
  comparison below is provisional until repeated targeted validation.
- Immediate retained normally sampled fixed-eight result is
  **`work/pyperformance/comparison-20260819-185353-AwqE0f`**, official
  stock **0.6672361371916246x**. Actual Apply source-native code totals
  **23,159,960 bytes / 1,524,970 blocks**, with **365,000 hidden
  trampoline bytes**, **3,970 total JIT source rows, including adapters /
  2,040 direct-function-body rows**, and unchanged **2,866 typed blocks /
  204 functions** across **80 Apply PIDs**.
- First candidate normally sampled fixed-eight comparison
  **`work/pyperformance/comparison-20260819-194906-NDDKWv`** completes all
  eight workloads against mode-matched retained **185353**. Official
  stock score declines **0.6672361371916246x → 0.6545178502099592x**;
  official changed/previous SOAC is **0.9700065211876199x**, below
  parity. All **80 actual Apply worker PIDs / 3,970 total JIT rows,
  including adapters / 2,040 direct-function-body rows** preserve every
  ordinary source identity, native byte, and machine block exactly:
  **23,159,960 bytes / 1,524,970 blocks / 2,866 typed blocks / 204
  functions**. Hidden trampolines grow **365,000 → 386,920 bytes
  (+21,920 / +6.00548%)**; **40,525 structured events** contain **zero
  ERROR or CRITICAL** records.
- Robust first-candidate `deltablue` changes **2.325320 → 2.349034 ms**,
  raw **0.989905x (95% interval 0.971221–1.008988x)**. Stock CPython
  itself changes **1.534509 → 1.447951 ms**, and the resulting
  stock-adjusted ratio is **0.934066x (0.908185–0.995850x)**. The raw
  interval includes parity, but the paired interval is wholly below one:
  this is a **significant paired deltablue regression**, not a win.
- The first normal deltablue paired comparison also has a material
  baseline-draw caveat: its retained stock **1.534509 ms** is
  approximately **7.8% slower** than clean retained three-round stock
  **1.423614 ms**, whereas candidate normal stock **1.447951 ms** is
  closer to that stable reference. Thus the adverse paired **0.934066x**
  is partly contaminated by a slow retained-stock draw. Candidate normal
  SOAC **2.349034 ms** is approximately **5.1% faster** than clean
  retained targeted SOAC **2.468877 ms**, but this cross-cohort
  comparison is **not a performance claim**. A fresh matched three-round
  targeted comparison is required before rejecting, refining, or
  retaining the candidate.
- `richards` changes **23.510022 → 23.731709 ms**, raw
  **0.990659x (0.953996–1.034373x)** and stock-adjusted
  **1.035576x (0.990682–1.083649x)**; one **30.10-ms** worker outlier
  and both parity-crossing intervals make it **INCONCLUSIVE**.
  `chaos` changes raw **0.935444x** / paired **0.951895x**.
  `comprehensions` raw **0.955750x (0.923671–0.972739x)** adjusts to
  **0.992854x**, near parity. `float` raw
  **0.972364x (0.938549–0.992303x)** and `spectral_norm` raw
  **0.952065x (0.882840–0.984228x)** are concerning unadjusted declines;
  none establishes a candidate improvement.
- Refined one-branch normally sampled fixed-eight comparison
  **`work/pyperformance/comparison-20260819-201359-gbo734`**, against
  retained **185353** and rejected first **194906**, completes all eight
  workloads. Official stock score improves provisionally to
  **0.6694448241941483x**, versus retained **0.6672361371916246x** and
  rejected first **0.6545178502099592x**; official previous SOAC is
  **1.0016222298324013x**. Every **80 actual Apply PIDs / 3,970 total
  JIT source rows, including adapters / 2,040 direct-function-body
  rows**, exact source identities, ordinary **23,159,960 native bytes /
  1,524,970 blocks**, and **2,866 typed blocks / 204 functions** are
  unchanged. Hidden trampoline bytes progress retained **365,000 →
  rejected first 386,920 → refined 381,080**; all **40,524 structured
  events** are INFO with **zero errors**.
- Refined normal `richards` changes retained **23.5100 → 22.4035 ms**,
  raw **1.049392x (95% interval 1.025988–1.088743x)** and paired-stock
  **1.089466x (1.048083–1.130195x)**. Against the rejected first
  implementation it is **1.059287x (1.02988–1.09449x)**. These normal
  fixed-eight observations are **PROVISIONAL**, not repeated-workload or
  retention evidence.
- Refined normal `deltablue` changes retained
  **2.325320 → 2.275270 ms**, raw **1.021997x
  (0.977194–1.047909x)**, which crosses parity. Its paired **0.9463x**
  is contaminated by the anomalously slow retained normal stock
  **1.5345 ms** versus refined stock **1.4208 ms**, almost identical to
  clean retained targeted stock **1.4236 ms**; do not interpret the
  polluted ratio as definitive regression or the raw point as a win.
- Refined normal `chaos` against retained is **INCONCLUSIVE:
  0.97619x (0.937–1.053x)**, but improves against the rejected
  four-branch candidate **1.043558x (1.008753–1.080586x)**, consistent
  with branch-overhead rescue without proving retained-baseline
  neutrality. `comprehensions` against retained is
  **0.95549x (0.90025–0.99993x)** and against rejected first
  **0.99973x**; this possible guardrail regression must be adjudicated
  with clean, independently repeated targeted rounds.
- Immediate retained clean targeted three-round result is
  **`work/pyperformance/comparison-20260819-185725-iJQ74K`**, official
  stock **0.5139251222980681x**. Actual Apply source-native code totals
  **54,686,760 bytes / 3,596,430 blocks**, with **746,520 hidden
  trampoline bytes**. Its **120 Apply PIDs / 10,650 total JIT source
  rows, including adapters / 5,490 direct-function-body rows**, preserve
  **2,265 typed blocks / 183 functions**.
- Definitive first-candidate clean three-round targeted comparison
  **`work/pyperformance/comparison-20260819-195448-0sGO85`**, against
  retained **185725**, reports official stock
  **0.517936448506483x** versus retained **0.5139251222980681x** and
  previous-SOAC **1.002659384469556x**. All **120 actual Apply PIDs /
  10,650 total JIT rows, including adapters / 5,490 direct-function-body
  rows** retain exact source identities, ordinary native bytes
  **54,686,760**, machine blocks **3,596,430**, and typed coverage
  **2,265 blocks / 183 functions**. Hidden trampoline code grows
  **746,520 → 789,720 bytes (+43,200)**. All **100,205 structured
  events** are INFO with **zero errors**.
- Clean repeated `deltablue` changes **2.468877 → 2.426780 ms**, raw
  **1.017347x (95% interval 0.991263–1.038226x)** and paired-stock
  **1.047992x (1.019526–1.078164x)**. Raw round ratios are
  **1.02353x / 0.98520x / 1.04664x**, so its raw interval crosses parity;
  paired rounds are **1.11833x / 1.02165x / 1.05280x**. This is a
  significant paired delta improvement but not a significant raw
  improvement and does not justify ignoring a separate real regression.
- Clean repeated `richards` is **NEUTRAL**: raw
  **1.008622x (0.998597–1.023029x)** and paired
  **1.010868x (0.994932–1.038805x)**. `comprehensions` is likewise
  **NEUTRAL at 1.000556x**.
- Crucially, `chaos` has a genuine repeated regression: raw
  **0.970913x (0.953508–0.985179x)** and paired-stock-adjusted
  **0.975258x (0.957298–0.992206x)**, with all three rounds below parity
  at approximately **0.980x / 0.962x / 0.960x**. Existing source native
  bodies are exactly unchanged, while every call pays the candidate's
  additional vectorcall guard. Worker setup also increases: `chaos`
  approximately **517 → 541 ms**, and `richards` approximately
  **676 → 727 ms**. The modest official subset aggregate
  **1.002659384469556x** does not outweigh this statistically significant
  mixed-workload regression. **REJECT THE FIRST ITERATION AS-IS**; do
  not run a final gate or record a retained performance change for it.
- The first-candidate lossless `deltablue` capture is
  **`work/logs/inline-native-recursion-stack-guard-first-deltablue_speedscope.json`**:
  **176 samples / 600 replay loops / 99 Hz / disabled block maps / zero
  lost samples**. Against an available **246-sample older-revision**
  comparison profile, overall recursion-related stack ancestry changes
  **8.12950% → 0.56815%**. The older profile contains
  **7.31615 percentage points** of exact-vectorcall public/helper
  ancestry plus **0.81335 points** of unrelated
  `RichCompare` / `BinaryConstraint.input` recursion; the candidate has
  **zero exact-trampoline public/helper samples**, and its remaining
  **0.56815%** is entirely that unrelated operation.
- Separate diagnostic frame shares change exact-trampoline self
  **7.31615% → 5.11333%** and thread-state acquisition
  **6.09612% → 2.27259%**. The baseline is **not the same retained
  revision**, sample counts are limited, and inclusive stack parents
  overlap; do not add these shares, treat their differences as
  independent gains, or claim exact matched-revision causality. The
  profile does establish that the targeted public/helper call disappears
  even though repeated `deltablue` raw performance is neutral and
  `chaos` regresses. Replacement guard overhead motivates a narrower
  hot-path refinement.
- Definitive refined one-branch clean targeted comparison
  **`work/pyperformance/comparison-20260819-201901`**, against retained
  **185725** and rejected first **195448**, reports official stock
  **0.525149227454957x** versus retained **0.5139251222980681x**;
  official previous SOAC is **1.0374660673409746x**. Every **120 actual
  Apply PIDs / 10,650 total JIT source rows, including adapters / 5,490
  actual direct-function-body rows**, exact source identities, ordinary
  **54,686,760 native bytes / 3,596,430 machine blocks**, and **2,265
  typed blocks / 183 functions** are unchanged. Hidden trampolines
  progress retained **746,520 → rejected first 789,720 → refined 777,240
  bytes**. All **100,206 structured events** are INFO with **zero
  errors**.
- Refined targeted `deltablue` significantly improves retained
  **2.468877 → 2.318359 ms**, raw
  **1.064924x (95% interval 1.030268–1.086195x)** and paired-stock
  **1.073872x (1.036578–1.107536x)**, with every independently
  alternated raw round above parity at **1.0822x / 1.0285x / 1.0606x**.
  It also improves versus the rejected first guard
  **1.046766x (1.020557–1.061684x)**.
- Refined targeted `richards` significantly improves retained
  **23.625606 → 21.780125 ms**, raw
  **1.084732x (1.056110–1.098710x)** and paired-stock
  **1.072466x (1.040196–1.096731x)**; versus rejected first it improves
  **1.075459x (1.046462–1.088537x)**.
- Refined targeted `chaos` is **NEUTRAL** versus retained,
  **40.004724 → 40.003514 ms**, raw
  **1.000030x (0.985100–1.032427x)** and paired-stock
  **0.974080x (0.958827–1.007537x)**, whose interval crosses parity.
  Against the genuinely regressing rejected first candidate it improves
  **1.029990x (1.017646–1.065572x)**, confirming branch-elision rescue
  without claiming an improvement over retained. `comprehensions` is
  likewise **NEUTRAL**, raw **1.019015x (0.992599–1.037186x)** and
  paired **0.981810x (0.955794–1.004346x)**. Approximate worker setup
  changes retained → refined `chaos` **517 → 500 ms**, `deltablue`
  **602 → 570 ms**, and `richards` **676 → 648 ms**; setup is diagnostic,
  not the measured benchmark headline.
- Matched lossless `deltablue` native profiles compare refined **178
  samples** against rejected first **176 samples** under the same
  **600 replay loops / 99 Hz / zero lost samples**. Exact-vectorcall
  public recursion-helper samples are **zero in both**; the available
  older retained profile had **7.31615%** helper ancestry. After removing
  the three unnecessary hot branches, exact-trampoline **self** falls
  **5.11333% → 2.80919%**; separately attributed thread-state
  acquisition falls **2.272591% → 0.561837%** but remains present; the
  older retained profile observed **6.096123%**. Unrelated `RichCompare`
  recursion changes older retained **0.813349% → rejected first
  0.568148% → refined zero sampled**. Sample
  counts are limited; these are distinct diagnostic attribution views,
  not additive nested stack shares or independent speedup predictions.
- True matched retained / refined lossless `richards` captures contain
  **244 / 226 samples**, the same **100 replay loops / 99 Hz**, and
  **zero lost samples**. Strict nearest-parent exact-trampoline public
  recursion-helper ancestry falls **2.459410% → zero**; exact-trampoline
  **self** falls **10.245541% → 7.081328%**; exact-trampoline live
  thread-state TLS acquisition falls **1.639606% → 0.884416%** and
  remains present. Refined unrelated `RichCompare` recursion contributes
  **0.442208%**. These are separate attribution views; limited samples
  prohibit adding overlapping ancestry or claiming TLS acquisition was
  removed.
- The retained targeted median is **23.625606 ms richards** and
  **2.468877 ms deltablue**. The completed first candidate has raw /
  stock-adjusted worker-level three-round evidence, approximate setup
  overhead, and **+43,200 repeated hidden bytes**; its real paired/raw
  chaos regression requires rejection. The first-candidate lossless
  deltablue capture confirms exact-trampoline helper elimination but has
  an older-revision comparator. Refined clean repeated benchmarking and
  matched first-versus-refined lossless deltablue profiling now confirm
  significant target gains, neutral guardrails, unchanged ordinary source
  bodies, and reduced actual trampoline overhead.
  Mode-matched first-candidate smoke proves exact ordinary
  source/direct-body invariance and **+2,192 hidden bytes**; rejected
  first normal fixed-eight repeats exact ordinary-body invariance with
  **+21,920 hidden bytes** and a below-parity official previous score.
  Refined smoke preserves the same exact ordinary bodies while reducing
  hidden growth to **+1,608 bytes** against retained, **584 bytes less**
  than the rejected first candidate. Refined normal fixed-eight likewise
  preserves every ordinary body, reduces hidden normal bytes from rejected
  **386,920** to **381,080**, and improves official stock / previous
  scores. Definitive clean targeted comparison further reduces rejected
  first hidden bytes **789,720 → 777,240** while improving both target
  workloads and restoring neutral mixed-workload behavior.
- Independently generate fresh Profile evidence for the retained and
  candidate SOAC revisions; do not reuse a counter dump across compiler
  revisions. Headline results must be normal Apply mode without attached
  `perf`; separate matched lossless native captures explain the result.
- Module-selection policy remains the repo-default pyperformance
  benchmark source roots. Standard-library `math` / `random` and other
  unselected dependencies remain stock unless independently verified;
  benchmark completion does not establish transformed meaningful hot
  code. Inspect exact actual source identities, direct bodies, hidden
  adapters/trampolines, errors, and transformed dependencies per worker.
- Candidate release smoke confirms eight-workload completion, exact
  source-body invariance, zero structured errors, and hidden-code growth.
  Normally sampled fixed-eight confirms the same exact source-body
  invariance and hidden growth but exposes regression signals. Clean
  repeated targeted completion proves a statistically significant chaos
  regression and rejects the first candidate as-is. The subsequent
  refined release smoke and normally sampled fixed-eight again confirm
  all eight workloads, exact ordinary source-body invariance, zero
  errors, and smaller hidden growth. Definitive repeated refined
  comparison confirms significant deltablue / richards improvements,
  neutral chaos / comprehensions, and exact ordinary source-body
  invariance; matched lossless deltablue profiling confirms smaller
  trampoline self overhead.
  The authoritative correctness gate independently passes; transformed
  dependency / standard-library coverage, workload-site recursion
  coverage, and full-suite stock-speedup acceptance remain unmeasured.

## Measurements

| Metric | Retained baseline | Candidate | Interpretation |
| --- | --- | --- | --- |
| Fixed-eight official stock score | 0.6672361371916246x | rejected first 0.6545178502099592x; refined 0.6694448241941483x | final refined normal / definitive targeted evidence preserved; full-suite 1.10x target remains unmet |
| Clean targeted fixed-four stock score | 0.5139251222980681x | rejected first 0.517936448506483x; refined 0.525149227454957x | refined target previous SOAC 1.0374660673409746x; first candidate remained rejected for real chaos regression |
| Changed SOAC / previous SOAC | retained results above | rejected first 0.9700065211876199x fixed-eight / 1.002659384469556x targeted; refined 1.0016222298324013x fixed-eight / 1.0374660673409746x targeted | first candidate rejected; final refined FULLY VALIDATED / RETAIN LANDING CANDIDATE |
| Targeted retained richards / deltablue elapsed | 23.625606 ms / 2.468877 ms | rejected first richards neutral / deltablue 2.426780 ms; refined richards 21.780125 ms / deltablue 2.318359 ms | refined richards raw 1.084732x / paired 1.072466x; delta raw 1.064924x / paired 1.073872x; both significant |
| Matched retained / refined richards recursion ancestry | retained 244 samples / 100 loops / 99 Hz / zero loss; exact-trampoline public helper 2.459410% | refined 226 samples / same loops / 99 Hz / zero loss; public helper zero | exact-trampoline self 10.245541% → 7.081328%; strict live-tstate TLS 1.639606% → 0.884416% remains; unrelated refined RichCompare 0.442208%; no nested sums |
| Earlier deltablue recursion ancestry / first candidate | older revision 246 samples / 8.129496%; exact trampolines 7.316147% | first candidate lossless 176 samples / 600 loops / 99 Hz; total 0.56815%; exact trampoline helper zero | different baseline revision; residual is unrelated RichCompare; no nested-share addition or exact causal claim |
| Matched first / refined deltablue trampoline profile | rejected first 176 samples / 600 loops / 99 Hz / zero loss | refined 178 samples / same loops / 99 Hz / zero loss; targeted public helper zero in both | strict exact-trampoline self 5.113329% → 2.809185%; live-tstate TLS 2.272591% → 0.561837% remains; older retained revision 6.096123%; no nested sums; sample caveat |
| Comprehensions recursion / trampoline ancestry | 0.684554% / 0.342277% | pending | low-share control; not a promised improvement |
| Smoke source-native bytes / blocks / hidden trampoline bytes | 2,238,412 / 147,769 / 36,500 | rejected first 2,238,412 / 147,769 / 38,692; refined 2,238,412 / 147,769 / 38,108 | each GREEN 8 / 8; all 397 total JIT rows / 204 direct bodies invariant; first hidden +2,192 / +6.00548%; refined +1,608 / +4.4055% retained and −584 / −1.5094% first; cold timing invalid |
| Normally sampled source-native bytes / blocks / hidden bytes | 23,159,960 / 1,524,970 / 365,000 | rejected first 23,159,960 / 1,524,970 / 386,920; refined 23,159,960 / 1,524,970 / 381,080 | all 3,970 JIT rows / 2,040 direct bodies invariant; refined hidden +16,080 retained / −5,840 first |
| First normal deltablue raw / paired-stock ratio | 2.325320 ms retained SOAC | 2.349034 ms; raw 0.989905x [0.971221, 1.008988]; paired 0.934066x [0.908185, 0.995850] | significant paired regression while stock improves 1.534509 → 1.447951 ms |
| First normal richards raw / paired-stock ratio | 23.510022 ms retained SOAC | 23.731709 ms; raw 0.990659x [0.953996, 1.034373]; paired 1.035576x [0.990682, 1.083649] | INCONCLUSIVE; one 30.10-ms outlier; no gain claim |
| Refined normal richards raw / paired-stock ratio | 23.5100 ms retained SOAC | 22.4035 ms; raw 1.049392x [1.025988, 1.088743]; paired 1.089466x [1.048083, 1.130195] | vs rejected first 1.059287x [1.02988, 1.09449]; provisional single-comparison evidence |
| Refined normal deltablue / guardrails | 2.325320 ms retained SOAC | 2.275270 ms; raw 1.021997x [0.977194, 1.047909]; paired 0.9463x contaminated by 1.5345 → 1.4208 ms stock draw | chaos retained 0.97619x [0.937, 1.053], vs first 1.043558x [1.008753, 1.080586]; comprehensions retained 0.95549x [0.90025, 0.99993], vs first 0.99973x; clean repeat required |
| Targeted source-native bytes / blocks / hidden bytes | 54,686,760 / 3,596,430 / 746,520 | rejected first 54,686,760 / 3,596,430 / 789,720; refined 54,686,760 / 3,596,430 / 777,240 | all 10,650 total JIT rows / 5,490 direct bodies invariant; refined hidden +30,720 retained / −12,480 first |
| First clean repeated deltablue raw / paired-stock ratio | 2.468877 ms retained SOAC | 2.426780 ms; raw 1.017347x [0.991263, 1.038226]; paired 1.047992x [1.019526, 1.078164] | first paired improves; raw not significant; one raw round below parity |
| Refined clean repeated deltablue raw / paired-stock ratio | 2.468877 ms retained SOAC | 2.318359 ms; raw 1.064924x [1.030268, 1.086195]; paired 1.073872x [1.036578, 1.107536] | all three rounds improve; vs rejected first 1.046766x [1.020557, 1.061684] |
| First / refined clean repeated richards raw / paired-stock ratio | 23.625606 ms retained SOAC | rejected first raw 1.008622x / paired 1.010868x neutral; refined 21.780125 ms, raw 1.084732x [1.056110, 1.098710], paired 1.072466x [1.040196, 1.096731] | final significantly improves retained and rejected first 1.075459x [1.046462, 1.088537] |
| First / refined clean repeated chaos raw / paired-stock ratio | 40.004724 ms retained SOAC | rejected first raw 0.970913x / paired 0.975258x regresses; refined 40.003514 ms, raw 1.000030x [0.985100, 1.032427], paired 0.974080x [0.958827, 1.007537] | FIRST ITERATION REJECTED; refined neutral vs retained / 1.029990x [1.017646, 1.065572] vs first |
| Refined clean repeated comprehensions guardrail | retained baseline | raw 1.019015x [0.992599, 1.037186]; paired 0.981810x [0.955794, 1.004346] | NEUTRAL; both intervals include parity |
| Optimized typed coverage fixed-eight / targeted | 2,866 blocks / 204 functions; 2,265 blocks / 183 functions | exactly unchanged in normal and targeted workers | final refined hidden-only trampoline change improves targets / restores neutral chaos |
| Serialized pre-optimization BlockPy bytes | unavailable for this retained snapshot | pending | do not invent unavailable instrumentation |
| Structured production Cranelift hot-guard decision | genuine production-used RED; frame-pointer reads 0 versus required 1; pinned offsets 80 / 88 / 104 pass | historical narrower 49,152-byte GREEN 1 passed / 574 filtered; final package-formatted 98,304-byte production guard independently GREEN | real FunctionBuilder / imported helper / shared exact-generic trampoline; 2 trusted loads; 1 cold / 0 hot helper calls; 25.83 s first compile is workflow-only |
| Refined three-branch-elision structured production decision | genuine unchanged-first-candidate production-CFG RED; hot Opcode::Brif count 4 versus required 1 | package-formatted refined actual production emitter GREEN; exactly 1 hot conditional branch | 2 trusted loads; pinned 80 / 88 / 104; unsigned 98,304; 1 unchanged cold public helper; unsupported-architecture fallback |
| Stock/transformed recursion semantics | independently verified unchanged-production GREEN 1 passed / 1.59 s, outer 1.794 s; no CPython bug | historical rejected first guard GREEN 1 / 1.63 s; current one-branch refinement GREEN 1 / 1.60 s | actual Profile → Verify → Apply; exact/generic calls, bounded recursion, 3 live threads, C callbacks, finalizers, margin; 23.92 s first one-time debug build is workflow-only |
| Full JIT / optimizer / typed-IR libraries | retained baseline passed | fresh refined GREEN 575 / 575 JIT; 214 / 214 optimizer; 54 / 54 typed IR | final package-formatted one-branch guarded production candidate |
| Broad transformed compatibility / scoped checks | retained baseline passed | fresh refined GREEN 17 / 17 in 40.06 s; combined optimizer/JIT cargo check --tests and scoped formatting / format-check GREEN | historical rejected first guard passed 17 / 17 in 40.41 s; final authoritative full gate also GREEN |
| Authoritative full `just test-all` gate | integrated prior change passed | GREEN 1,235 Python nodeids / 98 isolated batches / 8 workers / 98 passed / 0 failed; JIT 575, optimizer 214, typed 54, lowering 371, PyO3 8 | build runtime 1.571 s; test-target compile 36.23 s; cargo tests 62.855 s; inner / outer pytest 79.523 / 79.538 s; total test phase 142.405 s; candidate fully validated, landing pending |

## Attempt history

### Attempt 1: conservative shared-vectorcall native stack guard

- Change: save an independently host-reviewed two-file production change
  in the shared exact/generic vectorcall emitter. It keeps the existing
  first live thread-state lookup, reads the pinned embedded-base-frame
  soft-limit slot, uses the actual native frame pointer, and places the
  unchanged CPython recursion helper on the conservative cold path. Null
  state/base and zero soft limit use that helper; the initial
  `UINTPTR_MAX` soft-limit sentinel may safely skip it when pinned CPython
  would return zero. Private mirrors / tests pin the supported 64-bit
  `aarch64` / `x86_64` layout; arbitrary mismatches are not detected.
- Measurements and coverage: retained smoke, normally sampled fixed-eight,
  and clean repeated fixed-four artifacts are recorded above. Candidate
  release smoke passes **8 / 8 actual Apply workers**, preserving all
  **397 source rows / 204 direct bodies**, ordinary native bytes and
  blocks, and typed coverage; hidden trampolines grow **36,500 → 38,692
  bytes (+6.00548%)**, with zero errors in **3,816 events**. Cold smoke
  timing is not a performance measurement. First normal fixed-eight
  completes **8 / 8**, official stock **0.6545178502099592x** /
  previous SOAC **0.9700065211876199x**, with exact ordinary-body
  invariance, **+21,920 hidden bytes**, and a significant paired
  deltablue regression. Definitive repeated target stock is
  **0.517936448506483x**, previous SOAC **1.002659384469556x**;
  paired deltablue improves **1.047992x** but raw delta and richards are
  neutral, while `chaos` significantly regresses raw **0.970913x** /
  paired **0.975258x** in all three rounds. Ordinary native bodies stay
  identical; repeated hidden code grows **746,520 → 789,720 bytes**.
  Reject this first implementation as-is.
- Compatibility and tests: pinned CPython's exact downward-stack interval,
  release margin, embedded thread-state layout, and vendored vectorcall
  margin test are source-verified. The frozen actual stock/transformed
  **Profile → Verify → Apply** compatibility integration is independently
  **GREEN on unchanged production: 1 passed / 1.59 seconds**, including
  safely bounded recursion errors, live main / two-worker thread state,
  native stack protection, Python/C callbacks, profiling, finalizers,
  generic call fallbacks, hot counters, and generated native bodies. The
  first traceback-retention refcount mismatch was fixture-only and fixed
  with scoped exception cleanup. Genuine actual production-used Cranelift
  structured RED is established: frame-pointer reads **0 versus 1**
  after real pinned layout offsets **80 / 88 / 104** pass; its
  **25.83-second compilation** is workflow-only. The same actual
  production-emitter decision initially turns **GREEN: 1 passed / 574
  filtered**, with two real trusted loads, the initial narrower
  **49,152-byte** unsigned interval, and exactly one cold / zero hot
  existing public-helper calls. The subsequently accepted maximum-margin
  **98,304-byte** refinement has the same hot branch/instruction count;
  its final package-formatted real production-emitter structured
  regression independently passes. The frozen actual stock/transformed
  Profile → Verify → Apply final candidate integration is independently
  **GREEN: 1 passed / 1.63 seconds**; its one-time **23.92-second**
  debug-extension build is workflow-only. Full JIT / optimizer / typed-IR
  libraries are **GREEN 575 / 214 / 54**, broad transformed compatibility
  is **GREEN 17 / 17 in 40.41 seconds**, and combined test-target /
  scoped formatting checks are **GREEN**. Root-owned release smoke is
  **GREEN 8 / 8**; first normal fixed-eight is **NEGATIVE /
  INCONCLUSIVE**, and clean repeated targeted measurement establishes a
  significant real `chaos` regression. The **FIRST ITERATION IS
  REJECTED AS-IS**. Its first lossless 176-sample `deltablue` profile
  verifies exact-trampoline helper absence, but the available 246-sample
  comparator is an older revision. At this historical first-iteration
  checkpoint, a current-thread-invariant guard refinement remained
  pending; the subsequent completed source / structured / compatibility
  refinement is recorded below. No full gate or performance-log entry
  is appropriate for the rejected implementation. No user-visible
  CPython baseline mismatch is claimed.
- Historical refinement hypothesis after definitive repeated evidence:
  the first implementation pays **three presence-check branches** for
  live thread state, embedded base frame, and nonzero soft limit. The
  completed source proof and structured / transformed validation below
  establish that these are redundant attached-vectorcall invariants and
  reduce four hot branches to one without changing current-thread,
  sentinel, stack-boundary, error, or fallback semantics. The
  three-presence-branch first implementation remains rejected; the
  refined implementation's performance and retention remain pending.
- Rejected alternatives: **9.246% inclusive comprehensions ancestry**
  lacks a sound bounded implementation; inherited-method direct lookup
  needs **five to seven** production files and unresolved shadow/
  invalidation guards. Neither alternative supplies evidence of a safe
  retained speedup.
- Result: **IN PROGRESS; unchanged-production transformed compatibility
  GREEN; genuine production-used structured optimization RED → historical
  first narrower-guard GREEN → final maximum-margin structured GREEN;
  actual final candidate transformed compatibility GREEN 1 passed /
  1.63 seconds; full JIT 575 / optimizer 214 / typed IR 54, broad
  transformed 17 / 17, combined test-target and scoped formatting checks
  GREEN; root-owned release smoke GREEN 8 / 8 with invariant ordinary
  source bodies and +6.00548% hidden trampoline bytes; first normally
  sampled fixed-eight previous SOAC 0.9700065211876199x; definitive
  repeated paired deltablue 1.047992x but real paired/raw chaos
  regression 0.975258x / 0.970913x; FIRST ITERATION REJECTED AS-IS;
  subsequent one-branch refinement documented below; no full-gate
  success, retained candidate, performance-log entry, or full-suite
  acceptance claimed**.
- Reason: the source-backed redundant trampoline recursion / TLS path is
  real, but exact stack-boundary safety and measured general-purpose
  benefit must be proved before retention.

### Attempt 1 refinement: remove proven redundant vectorcall presence guards

- Chronology: this is a refinement of the **same** native-recursion
  strategy after the fully recorded first candidate was rejected for a
  real repeated `chaos` regression. A new genuine unchanged-first-
  candidate production-emitter structured **RED** is verified, and the
  package-formatted refined production implementation turns it **GREEN**.
  Its frozen actual stock/Profile → Verify → Apply transformed
  regression is independently **GREEN: 1 passed / 1.60 seconds**.
  Fresh full Rust libraries, broad transformed compatibility, combined
  / scoped checks, and refined release smoke **8 / 8** are **GREEN**;
  hidden smoke bytes are **38,108**, versus retained **36,500** and
  rejected first **38,692**. Refined normal fixed-eight official stock /
  previous SOAC are **0.6694448241941483x / 1.0016222298324013x**,
  with **381,080 hidden bytes**. Definitive refined targeted stock /
  previous SOAC are **0.525149227454957x / 1.0374660673409746x**;
  repeated deltablue and richards significantly improve raw and paired,
  chaos and comprehensions remain neutral, and matched lossless delta
  profiling confirms lower trampoline overhead; true matched richards
  profiling also confirms eliminated public helper and lower trampoline
  self. Status is **FULLY VALIDATED / RETAIN LANDING CANDIDATE**;
  authoritative full correctness gate independently passes all
  **1,235 transformed Python nodeids / 98 isolated batches** and Rust
  suites. Candidate landing has not yet occurred.
- Independent pinned-CPython source proof establishes the preconditions
  at the real Python vectorcall boundary:
  `vendor/cpython/Include/internal/pycore_call.h:121` invokes the vectorcall
  function using an attached current `PyThreadState`, and
  `vendor/cpython/Objects/call.c:25` subsequently dereferences that
  state while checking the result. Existing generated direct callees and
  exception paths already dereference the same thread state
  unconditionally. A null state is not a supported Python-accessible
  input to this production entrypoint.
- `vendor/cpython/Python/pystate.c:1533` through the base-frame
  publication at approximately line **1556** initializes the embedded
  interpreter frame and assigns the nonnull `tstate->base_frame` pointer
  before exposing that state. Its attach path at approximately lines
  **2197–2214** initializes native stack protection whenever the hard
  limit is zero **before** publishing / attaching the current state.
- `vendor/cpython/Python/ceval.c:193–260` derives valid native stack
  bounds and a nonzero soft limit; the normal stack-protection set/reset
  operations preserve these bounds. Consequently, at this actual
  attached vectorcall entry, nonnull current thread state, nonnull base
  frame, and initialized nonzero soft limit are independently established
  CPython invariants rather than speculative profile observations.
- Implemented, package-formatted refinement: remove the **three redundant
  presence branches**
  for those already-proven properties. Keep the existing live thread
  state acquisition, exactly **two trusted pinned loads** at offsets
  **80 / 104**, embedded-frame size **88**, universal **32-KiB** guard
  interval **`[soft - 65536, soft + 32768)`**, wrapping unsigned
  **`(frame_pointer - soft + 65536) <u 98304`**, and the original
  authoritative public recursion helper as the **single cold branch**.
  The hot path should contain exactly **one unsigned interval branch**.
  Unsupported architectures retain their existing unconditional helper;
  current-thread ownership, wrong-fiber behavior, sentinel semantics,
  recursion errors, callbacks, and finalizers must remain unchanged.
- Authorized refinement scope is only existing production
  **`crates/soac_jit/src/jit/vectorcall.rs`** plus its existing
  **`#[cfg(test)]`-only `crates/soac_jit/src/jit/test.rs`** structured
  regression. The already-present private pinned mirrors in
  `runtime_context.rs` require **no further refinement change**. No
  exported helper, public API, IR operation, source direct-function body,
  cache, mutable global, or extra production file is introduced.
- The new genuine structured optimization **RED** now exercises the
  actual **unchanged first-candidate production emitter**. Its existing
  native frame-pointer, pinned offsets **80 / 88 / 104**, wrapping
  unsigned **98,304-byte** interval, and single cold public-helper
  assertions all pass first. Its **sole intended failure** counts hot
  Cranelift **`Opcode::Brif`: actual 4, expected 1**—the interval
  branch plus the three redundant presence checks. This is real
  production-CFG evidence, not a fabricated stub or a renderer test.
- The actual refined production-emitter structured regression now
  independently turns that genuine hot-branch **RED → GREEN: 4 → 1**.
  It preserves exactly **two trusted pinned loads**, offsets **80 / 104**,
  embedded-frame size **88**, the exact wrapping unsigned
  **98,304-byte** interval, exactly **one original cold public helper**,
  and the unsupported-architecture unconditional fallback. The runtime
  package was formatted **before** running the first refined transformed
  integration.
- Baseline and rejected-first-iteration semantics remain historically
  GREEN. The **refined** frozen actual stock/Profile → Verify → Apply
  transformed integration now independently passes **1 test in 1.60
  seconds**, preserving all existing exact/generic call, recursion,
  concurrent-thread, callback, finalizer, and instrumentation controls.
  Fresh full JIT / optimizer / typed-IR libraries independently pass
  **575 / 575**, **214 / 214**, and **54 / 54**; broad transformed
  compatibility passes **17 / 17 in 40.06 seconds**; combined
  optimizer/JIT test-target checks and package-scoped formatting /
  format-check are **GREEN**. Refined release smoke passes **8 / 8**,
  with exact ordinary source identities / bytes / blocks, **3,816 INFO /
  zero errors**, and hidden bytes retained **36,500 → rejected first
  38,692 → refined 38,108**. Refined normally sampled fixed-eight
  completes all **80 Apply PIDs**, preserving exact ordinary source
  bodies with hidden bytes **365,000 → 386,920 → 381,080**, official
  stock **0.6694448241941483x**, and previous SOAC
  **1.0016222298324013x**. Definitive targeted stock / previous SOAC are
  **0.525149227454957x / 1.0374660673409746x**, with significant
  repeated raw / paired `deltablue` **1.064924x / 1.073872x** and
  `richards` **1.084732x / 1.072466x**, neutral `chaos` /
  `comprehensions`, invariant ordinary native code, and hidden bytes
  **746,520 → 789,720 → 777,240**. Matched lossless first-versus-refined
  delta profiles show strict trampoline self **5.113329% → 2.809185%**;
  true matched retained / refined richards shows public helper
  **2.459410% → zero** and trampoline self **10.245541% → 7.081328%**.
  Debug-single timings are invalid for performance.
- The authoritative full **`just test-all`** gate independently exits
  **zero**; its complete evidence is
  **`work/logs/inline-native-recursion-stack-guard-test-all.log`**.
  Exactly **1,235 transformed pytest nodeids / 98 isolated batches / 8
  workers** complete **98 passed / zero failed**. Rust JIT passes
  **575 tests in 21.50 seconds**, optimizer **214 in 0.70 seconds**,
  typed IR **54 in 0.01 seconds**, lowering **371 in 1.54 seconds**, and
  the PyO3 extension **8 in 0.14 seconds**. Runtime build takes
  **1.571 seconds**, test-target compilation **36.23 seconds**, overall
  Cargo tests **62.855 seconds**, inner / outer transformed parallel
  pytest **79.523 / 79.538 seconds**, and the complete test phase
  **142.405 seconds**. The new native-recursion integration passes in
  **2.20 seconds**; the prior uniform-field regression passes in
  **2.59 seconds**. One **28-node counter shard takes 78.87 seconds**
  and dominates parallel pytest wall time.
- Verdict: **FULLY VALIDATED / RETAIN LANDING CANDIDATE; GENUINE
  ACTUAL-PRODUCTION HOT-BRANCH
  RED → GREEN 4 → 1; FIRST ITERATION REMAINS REJECTED AS-IS; refined
  transformed integration independently GREEN 1 passed / 1.60 seconds;
  fresh JIT 575 / optimizer 214 / typed IR 54, broad transformed 17 / 17
  in 40.06 seconds, combined test-target / scoped formatting checks GREEN;
  refined release smoke GREEN 8 / 8 with exact ordinary-body invariance
  and hidden bytes 36,500 → 38,692 → 38,108; refined normal fixed-eight
  official stock 0.6694448241941483x / previous SOAC
  1.0016222298324013x; definitive targeted stock 0.525149227454957x /
  previous 1.0374660673409746x, repeated deltablue raw 1.064924x /
  paired 1.073872x and richards raw 1.084732x / paired 1.072466x;
  chaos / comprehensions NEUTRAL; strict lossless delta trampoline self
  5.113329% → 2.809185%, matched richards public helper 2.459410% →
  zero; authoritative full gate GREEN 1,235 nodeids / 98 batches /
  98 passed; landing pending**.

## Verdict and next action

- Verdict: **FULLY VALIDATED / RETAIN LANDING CANDIDATE; actual
  unchanged-production transformed
  compatibility GREEN 1 passed / 1.59 seconds; no baseline CPython bug;
  genuine production-used structured RED actual native-frame reads 0
  versus expected 1 turns historical first-iteration GREEN 1 passed /
  574 filtered; accepted final maximum-margin interval
  [soft - 65536, soft + 32768), unsigned bound 98304, independently
  verified final real production-emitter structured GREEN; frozen actual
  candidate transformed Profile → Verify → Apply compatibility GREEN
  1 passed / 1.63 seconds; full JIT 575 / optimizer 214 / typed IR 54,
  broad transformed 17 / 17 in 40.41 seconds, combined cargo test-target
  and scoped format checks GREEN; release smoke GREEN 8 / 8, ordinary
  source-native 2,238,412 bytes / 147,769 blocks invariant, hidden
  trampolines 36,500 → 38,692 bytes; first normal fixed-eight official
  stock 0.6545178502099592x / previous SOAC 0.9700065211876199x,
  paired deltablue 0.934066x [0.908185, 0.995850] with a slow
  retained-stock-draw caveat; richards INCONCLUSIVE; hidden normal
  365,000 → 386,920 bytes; definitive clean targeted previous SOAC
  1.002659384469556x, paired delta 1.047992x [1.019526, 1.078164],
  richards NEUTRAL, chaos raw 0.970913x [0.953508, 0.985179] / paired
  0.975258x [0.957298, 0.992206] REGRESSION across all three rounds,
  hidden target 746,520 → 789,720 bytes; FIRST ITERATION REJECTED
  AS-IS; first lossless delta profile 176 samples / 600 loops proves
  zero exact-trampoline public/helper samples against an explicitly
  older 246-sample baseline; source-proved three-branch-elision
  refinement package-formatted; new genuine actual-production structured
  RED → GREEN hot Opcode::Brif 4 → 1 while preserving two trusted loads,
  interval 98,304, and one unchanged cold public helper; refined frozen
  actual stock/Profile → Verify → Apply transformed regression GREEN
  1 passed / 1.60 seconds; fresh refined JIT 575 / optimizer 214 / typed
  IR 54, broad transformed 17 / 17 in 40.06 seconds, combined test-target
  / scoped formatting checks GREEN; refined release smoke GREEN 8 / 8,
  exact ordinary-body invariance, hidden bytes retained 36,500 → rejected
  first 38,692 → refined 38,108; refined normal fixed-eight official
  stock 0.6694448241941483x / previous SOAC 1.0016222298324013x,
  hidden 365,000 → 386,920 → 381,080; definitive targeted stock
  0.525149227454957x / previous 1.0374660673409746x, all 10,650 total
  JIT rows including adapters / 5,490 direct bodies invariant, hidden
  746,520 → 789,720 → 777,240; repeated deltablue 2.468877 → 2.318359
  ms raw 1.064924x / paired 1.073872x, richards 23.625606 → 21.780125
  ms raw 1.084732x / paired 1.072466x, chaos / comprehensions NEUTRAL;
  matched lossless first 176 / refined 178 delta samples, helper zero in
  both, strict trampoline self 5.113329% → 2.809185%; matched retained
  / refined richards 244 / 226 samples, helper 2.459410% → zero,
  trampoline self 10.245541% → 7.081328%; authoritative full gate GREEN
  1,235 transformed nodeids / 98 isolated batches / 8 workers / 98
  passed / zero failed, JIT 575 / optimizer 214 / typed 54 / lowering
  371 / PyO3 8; landing pending; full-suite stock 1.10x goal unmet**.
- Transferable lesson: recursion-check profile ancestry is not permission
  to bypass CPython. A shortcut may skip an existing public checker only
  when the *current* native frame and *current* thread state prove that a
  conservatively widened native-stack interval cannot require it; the
  pinned initial sentinel may skip only when the original checker would
  also return zero, and supported layout assumptions must remain explicit.
- Next action: land the **FULLY VALIDATED / RETAIN LANDING CANDIDATE**
  after its independently successful authoritative full correctness
  gate. Preserve all rejected-first-
  implementation history. Full-suite stock **1.10x** remains unmet and
  unmeasured.
