# Strict-module builtin resolution and bigint correctness preconditions

- Date: August 20, 2026 PDT
- Status: Initial correctness prerequisites and both additional out-of-policy corrections retained; the new authoritative full gate passed.
- Strategy: Restore demonstrated CPython behavior before introducing authenticated strict modules or pursuing strict-only performance claims.

## Goal and hypothesis

Strict-module optimization requires an explicitly authenticated strict capability; ordinary Python modules must retain CPython-visible global mutation, builtin resolution, arbitrary-precision integer arithmetic, and existing fail-closed behavior for unsupported frame-sensitive operations. The repository currently has **no authenticated strict-module capability** and therefore **no valid strict-mode benchmark result**. Existing ordinary-SOAC performance comparisons do not establish progress toward the current strict-only objective.

The initial concrete hypothesis was that ordinary-source `len` was frozen as an SOAC runtime constant, while scalar-demand arithmetic incorrectly raised `OverflowError` when an intermediate escaped signed 64-bit range despite the mathematically correct final Python result fitting. The retained initial correction restores live `len` resolution and boxes integer arithmetic whenever an entire expression cannot be proven safe.

The user has subsequently clarified two **explicitly approved compatibility exceptions**: indexed fast stores may intentionally omit dictionary-watcher notifications, and known builtin snapshots may intentionally ignore ordinary module-global shadows introduced later. Neither exception is a correctness defect for this strategy, and preserving those fast paths is mandatory. The follow-on hypothesis concerns only behavior outside both exceptions: module globals already explicitly bound before a call, and custom builtins mappings already captured when a function is created.

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

## Follow-on genuine RED: already-bound globals and captured builtins

After the initial corrected full gate, two additional matched stock-versus-SOAC integration cases genuinely failed:

1. For an already initialized module containing `globals = replacement`, where `replacement()` returns `41`, CPython calls the existing module binding and returns **41**. SOAC instead treats the source spelling as its compiler-owned `globals()` intrinsic and returns the **module dictionary**.
2. For `__builtins__ = {"ord": lambda value: 41}` established before function creation, CPython resolves `ord` through that already-captured custom mapping and returns **41**. SOAC instead snapshots its ordinary builtin implementation and returns **97** for `ord("a")`.

These are not later introductions of an undeclared ordinary builtin shadow, and neither case relies on dictionary watchers. Both matched Python regressions went **RED before the production correction and GREEN afterward**.

The retained correction removes the source-spelling-only `globals()` rewrite, allowing an already declared module binding to stay an ordinary global. Authenticated compiler-generated `RuntimeName::Globals` operations retain their dedicated behavior. Both legacy and typed JIT helper classifiers now recognize `Globals` only from an authenticated runtime name, runtime-name module constant, or existing validated helper facts; a mutable source global named `globals` is no longer intrinsically trusted.

Known-builtin snapshots are disabled only within a module explicitly declaring its own `__builtins__` mapping, so source names such as `ord` resolve through the function's already-captured mapping. Normal modules retain their existing builtin snapshots and specialized `ord` primitive. Frame-sensitive `globals`, `locals`, `eval`, and `exec` keep their explicit runtime safeguards, and compiler-owned intrinsics remain unaffected. User-approved watcher-free indexed stores and later-undeclared builtin-shadow shortcuts remain unchanged.

Two structured lowering regressions and one structured JIT provenance regression also went **RED before correction and GREEN afterward**. The new authoritative full gate subsequently **PASSED** with **1,360 Rust tests** and **1,329 transformed Python nodeids across 104 passing isolated batches**.

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
- Follow-on already-bound module `globals`: **genuine RED**, stock **41** versus SOAC **module dictionary**, then **GREEN** with SOAC **41**.
- Follow-on initially captured custom builtin `ord`: **genuine RED**, stock **41** versus SOAC **97**, then **GREEN** with SOAC **41**.
- Follow-on structured lowering provenance regressions: **two RED, then two GREEN**.
- Follow-on structured JIT global-versus-intrinsic provenance regression: **one RED, then one GREEN**.
- Follow-on broad transformed Python validation: **33 passed**, including canonical `globals()`, frame-sensitive safeguards, class/import/bootstrap behavior, captured mappings, and thread coverage.
- Follow-on full Rust suites: `soac_lowering` **374 passed**, `soac_opt` **214 passed**, and `soac_jit` **587 passed**.
- Follow-on scoped formatting check and JIT package check including tests: passed.
- Follow-on new full `just test-all` gate: **PASSED** with **1,360 Rust tests across 71 test groups**, and **1,329 transformed Python nodeids across 104 passing batches / eight workers / zero failed batches**. The earlier **1,357 Rust / 1,327 Python** gate remains the separately recorded historical validation of the initial implementation.
- Follow-on full-gate timings: build **1.474 s**, Cargo tests **73.813 s**, pytest **77.895 s** internally / **77.908 s** externally, total test phase **151.733 s**.

## Benchmark, transformation, and native-code evidence

No authenticated strict-module runtime or compiler capability exists yet. Consequently:

- Strict transformed benchmark coverage: **unavailable**.
- Strict-versus-stock pyperformance result: **unavailable**.
- Previous-strict-SOAC versus candidate result: **unavailable**.
- Strict typed-IR, machine-code, JIT worker, and native-code-size measurements: **unavailable**.
- Full 97-driver / 124-result strict-suite acceptance: **not achieved**.

Conservatively boxing loop-carried integers may affect performance, but that consequence is **unmeasured**. Ordinary-module benchmarks are not valid evidence for the strict-only objective and must not be substituted for an authenticated strict benchmark. No ordinary-module throughput result or historical non-strict comparison is presented as strict-goal evidence.

## Remaining limitations and verdict

Approximately **145 other ordinary builtin names** remain on the preexisting frozen-runtime candidate list. Known-builtin snapshots that ignore later ordinary global shadows, and watcher-free indexed fast stores, are explicitly approved compatibility behavior and remain intact. The demonstrated follow-on defects were narrower: direct `globals()` ignored an already declared module binding, and ordinary builtin snapshots ignored a custom builtins mapping already bound at function creation. Both now pass matched stock-versus-SOAC regressions while preserving compiler-intrinsic provenance and the existing explicit frame-sensitive failure boundary. Their new full-suite gate also passed.

The relevant user-owned `doc/SPECIALIZATION.md` update remains pending and is deliberately outside this document-only lease. An authenticated strict capability, strict-only transformation evidence, and a valid strict benchmark all remain outstanding despite the corrected full gate passing.

**Verdict:** Retain the full-gate-verified `len`, bigint, and coherent scalar-planning prerequisites and the full-gate-verified corrections for already-bound `globals` and initially captured custom builtins. Preserve explicitly approved watcher-free and ordinary builtin-snapshot fast paths; reject blanket builtin rewrite removal. The follow-on full `just test-all` gate **PASSED**. Do not claim strict-module support, suite-wide optimization progress, a valid strict benchmark, or attainment of the 10%-over-stock goal.

## Transferable lessons

- Match stock and transformed behavior before inferring optimization safety from ordinary-source builtin names.
- Distinguish the first genuine failure from shared-mutex poisoning cascades during broad Rust validation.
- Preserve actual compiler provenance; source spelling alone cannot authenticate an intrinsic.
- Distinguish explicitly approved later-shadow and watcher omissions from genuinely incorrect behavior for already-bound globals or preexisting captured builtin mappings.
- Frame-sensitive builtins require callable-identity-aware dispatch or an explicit unsupported boundary.
- Use machine integers only where the complete Python arithmetic path, including intermediates, is proven safe.
- Keep scalar representation planning and native emission aligned on the same validated integer-range facts; focused suites alone did not expose their cross-pass mismatch.
