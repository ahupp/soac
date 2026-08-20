# Strict-module builtin resolution and bigint correctness preconditions

- Date: August 20, 2026 PDT
- Status: Correctness prerequisites retained; corrected full gate passed.
- Strategy: Restore demonstrated CPython behavior before introducing authenticated strict modules or pursuing strict-only performance claims.

## Goal and hypothesis

Strict-module optimization requires an explicitly authenticated strict capability; ordinary Python modules must retain CPython-visible global mutation, builtin resolution, arbitrary-precision integer arithmetic, and existing fail-closed behavior for unsupported frame-sensitive operations. The repository currently has **no authenticated strict-module capability** and therefore **no valid strict-mode benchmark result**. Existing ordinary-SOAC performance comparisons do not establish progress toward the current strict-only objective.

The concrete hypothesis was that ordinary-source `len` was incorrectly frozen as an SOAC runtime constant, while scalar-demand arithmetic incorrectly raised `OverflowError` when an intermediate escaped signed 64-bit range despite the mathematically correct final Python result fitting. Restoring live `len` resolution and boxing integer arithmetic whenever an entire expression cannot be proven safe should correct these genuine CPython divergences without weakening existing compatibility boundaries.

## Baseline and genuine RED evidence

Five matched stock-versus-SOAC external global-rebinding controls already passed before the production changes:

1. Direct module-attribute assignment.
2. Mutation through `module.__dict__`.
3. Mutation through `function.__globals__`.
4. `exec` against the module dictionary.
5. `ctypes.pythonapi.PyDict_SetItem` through the CPython C API.

Two ordinary builtin-resolution cases genuinely failed:

- Installing a module-global `len` after the first invocation produced the frozen builtin result instead of CPython's `(3, 41)`.
- A function's explicitly supplied, subsequently mutated `__builtins__` mapping was ignored instead of producing CPython's `(41, 52)`.

Three genuine arbitrary-precision regressions independently failed for addition, subtraction, and multiplication: their signed-64-bit intermediates raised SOAC `OverflowError`, while the matched vendored CPython evaluated each complete expression and returned `chr(0) == "\\x00"`.

The earlier `builtin_dynamic_global_shadow` integration case was already marked expected-failure for both transformed SOAC and entry-interpreter modes, confirming that the incorrect behavior was preexisting rather than introduced by this attempt.

## Rejected iteration: disable all frozen builtin rewrites

Disabling the entire ordinary-source runtime-builtin rewrite made the new precondition cases pass, but was rejected after actual broad validation:

- Full JIT suite: **454 passed, 132 failed**.
- The first real failure lowered `ValueError` to an ordinary global in an entry-interpreter test supplying a null module-globals pointer. Its panic poisoned a shared Python-runtime test mutex, causing approximately **105 secondary lock-poison failures**.
- Approximately **26 independent existing optimizer cases** relied on unguarded ordinary-source builtin assumptions for operations including `list`, `set`, `tuple`, `map`, `filter`, `iter`, `next`, and `range`.
- Five existing frame-sensitive controls stopped failing explicitly: canonical `locals`, `eval`, and `exec` observed an outer frame or otherwise bypassed the required `NotImplementedError` boundary.

This iteration demonstrates that the broad behavior cannot be removed safely as an incidental one-line change: authenticated builtin provenance, actual-callable guards, interpreter entry globals, frame-sensitive dispatch, and obsolete specialization expectations require a separate coordinated strategy.

## Retained implementation and compatibility analysis

The retained builtin correction excludes only ordinary-source `len` from the existing frozen-runtime-builtin candidate set. Existing global lookup consequently checks the current module dictionary and the function's actual captured builtins mapping on every applicable lookup. Structured lowering coverage confirms that `len` remains a live global while the previously established `print` and `range` paths are unchanged.

Compiler-generated runtime intrinsics remain distinct, including f-string conversion, `Globals`, `UnpackFixed`, and runtime bootstrap handling. Existing canonical `locals`/`eval`/`exec` operations continue to fail explicitly instead of observing the wrong Python frame.

The separate arithmetic correction keeps scalar machine arithmetic only when the relevant complete operation and intermediate values have proven safe ranges. Potentially overflowing intermediates fall back to boxed Python bigint operations, preserving CPython's arbitrary-precision result rather than raising an artificial signed-64-bit overflow.

After the genuine RED cases passed, the existing builtin-shadow fixture was run in **stock, SOAC, and entry modes: 3/3 passed**. Its obsolete transformed-mode expected-failure annotation was then removed.

## First full-gate failure: scalar block-parameter planning

The first complete `just test-all` run passed **all 1,357 Rust tests**, including the previously reported lowering, optimizer, and JIT suites. It then collected **1,321 transformed Python nodeids in 104 isolated batches across eight workers**: **100 batches passed and four genuinely failed**.

The four failures were:

1. `tests/test_regression_direct_exception_cleanup.py::test_apply_mode_direct_call_miss_uses_generic_fallback`.
2. `tests/test_regression_basic_blocks_while.py::test_basic_block_lowering_while_break_continue_else[soac]`.
3. `tests/test_regression_scalar_cleanup_root.py::test_profile_mode_scalar_cleanup_root_runs_with_unmaterialized_i64_retire`.
4. `tests/test_counter_dump_file.py::test_specialized_nested_generator_identity_iter_preserves_generator_state`.

Every failure reported a missing local mapping for a jump or branch block parameter that the planner still declared `ExactI64`. The bigint correction correctly made the emitter box arithmetic unless `IntFacts` prove every intermediate fits, but the existing planner continued to infer scalar block parameters merely from scalar-looking operands. The resulting sidecar/emitter disagreement broke ordinary loop-carried values, profiled cleanup roots, direct-call fallback joins, and specialized generator joins.

Restoring the earlier unknown-range scalar fallback is rejected: a dynamic Python integer can cross the signed-64-bit boundary, and assuming otherwise reintroduces the arbitrary-precision correctness bug. The retained coherent correction makes scalar block-parameter planning apply the **same `i64_binop_result_facts` helper and proven-range `IntFacts` rule as emission**. Constants carry their exact values; incoming scalar locals and scalar-return plans have unknown ranges; recursive addition, subtraction, and multiplication remain scalar only when that shared helper proves the complete operation safe. Otherwise the planner consistently selects boxed Python values for the block parameters and edge transports.

The old structured expectation that an unbounded loop-carried integer always remains `ExactI64` was replaced by a genuine boxed-phi and boxed-edge regression. Cleanup-root coverage likewise now verifies the correct transition from an initially empty slot to a possibly owned boxed value and preserves exit sweeping. Three loop-carried addition/subtraction/multiplication cases crossing signed-64-bit limits went RED before the coherent correction and GREEN afterward; three additional dynamic `ord`-dependent overflow cases protect the same CPython-bigint boundary. All four integration nodes that failed in the first full gate subsequently passed in the focused corrected run.

The separately rerun corrected full gate subsequently **PASSED**: all **1,357 Rust tests across 71 test groups** passed, followed by all **1,327 transformed Python nodeids in 104 isolated batches across eight workers**, with **104 passing batches and zero failures**. The six-node increase relative to the initial failed run reflects the additional dynamic and loop-carried bigint safeguards.

The first failed gate spent 85.073 seconds in pytest and 170.963 seconds in its test phase. This negative outcome must remain in the strategy history even after a later corrected gate succeeds.

## Validation

- Initial focused transformed Python coverage: **19 passed**, including the new preconditions, all five frame-sensitive fail-closed controls, and stock/SOAC/entry builtin-shadow coverage.
- Corrected focused transformed Python coverage: **23 passed; one preexisting unconditional `exec`-closure expected failure**, including all four initial full-gate failures and the additional bigint safeguards.
- Matched stock/SOAC dynamic-operand bigint safeguards: **three passed** for addition, subtraction, and multiplication.
- Matched stock/SOAC loop-carried bigint regressions: **three RED before correction, three GREEN afterward**.
- Structured scalar-phi, edge-transport, and boxed cleanup-root ownership regressions: passed.
- Full `soac_lowering` suite: **372 passed**.
- Full `soac_opt` suite: **214 passed**.
- Full `soac_jit` suite: **586 passed**.
- First full-gate Rust phase: **1,357 passed**.
- First full-gate transformed Python phase: **100 batches passed; four batches failed** on scalar block-parameter representation mismatches.
- Scoped formatting and package checks: passed.
- Corrected final full `just test-all` gate: **PASSED** with **1,357 Rust tests across 71 test groups**, and **1,327 transformed Python nodeids across 104 passing batches / eight workers / zero failed batches**.
- Corrected full-gate timings: build **1.492 s**, Cargo tests **62.201 s**, pytest **81.258 s** internally / **81.279 s** externally, total test phase **143.493 s**.

## Benchmark, transformation, and native-code evidence

No authenticated strict-module runtime or compiler capability exists yet. Consequently:

- Strict transformed benchmark coverage: **unavailable**.
- Strict-versus-stock pyperformance result: **unavailable**.
- Previous-strict-SOAC versus candidate result: **unavailable**.
- Strict typed-IR, machine-code, JIT worker, and native-code-size measurements: **unavailable**.
- Full 97-driver / 124-result strict-suite acceptance: **not achieved**.

Conservatively boxing loop-carried integers may affect performance, but that consequence is **unmeasured**. Ordinary-module benchmarks are not valid evidence for the strict-only objective and must not be substituted for an authenticated strict benchmark. No ordinary-module throughput result or historical non-strict comparison is presented as strict-goal evidence.

## Remaining limitations and verdict

Approximately **145 other ordinary builtin names** remain on the preexisting frozen-runtime candidate list. Direct `globals()` retains its existing special lowering, including unresolved late-shadow and alias-identity limitations. Comprehensive ordinary builtin correctness requires dynamically resolving the actual callable, authenticating genuine compiler-owned intrinsics, preserving explicit failures only for actual canonical frame-sensitive builtins, and updating the existing specialization assumptions without forgeable source spellings.

The relevant user-owned `doc/SPECIALIZATION.md` update remains pending and is deliberately outside this document-only lease. An authenticated strict capability, strict-only transformation evidence, and a valid strict benchmark all remain outstanding despite the corrected full gate passing.

**Verdict:** Retain the narrowly verified CPython-correct `len`, bigint, and coherent scalar-planning prerequisites; the corrected full gate passed. Reject the blanket builtin rewrite removal. Do not claim strict-module support, suite-wide optimization progress, a valid strict benchmark, or attainment of the 10%-over-stock goal.

## Transferable lessons

- Match stock and transformed behavior before inferring optimization safety from ordinary-source builtin names.
- Distinguish the first genuine failure from shared-mutex poisoning cascades during broad Rust validation.
- Preserve actual compiler provenance; source spelling alone cannot authenticate an intrinsic.
- Frame-sensitive builtins require callable-identity-aware dispatch or an explicit unsupported boundary.
- Use machine integers only where the complete Python arithmetic path, including intermediates, is proven safe.
- Keep scalar representation planning and native emission aligned on the same validated integer-range facts; focused suites alone did not expose their cross-pass mismatch.
