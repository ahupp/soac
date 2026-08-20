---
title: "CPython class static-attribute and shared-key layout parity"
---

# CPython class static-attribute and shared-key layout parity

- Status: RETAIN VALIDATED CPYTHON-CORRECTNESS / MIXED-INDEX CANDIDATE;
  RESIDUAL PARENT-RELATIVE PERFORMANCE REGRESSION DISCLOSED; FULL GATE GREEN
- Pacific selection time: 2026-08-19 23:50 PDT
- Integrated baseline: `srxzvruu d0655b62`; working change: `lwyqsqsm`
- Outcome: the original stock GREEN `8 / 8` / transformed RED `8 / 8` and
  genuine structured lowering RED are preserved below. The five-file
  compatibility candidate subsequently turns the real structured lowering
  regression GREEN `1 / 1` and all focused stock/transformed cases GREEN
  `16 / 16`. However, **all four existing owner-field specialization suites
  then genuinely FAIL**: CPython-preseeded split keys already exist before the
  current post-create watcher is installed, and the installed watcher does not
  replay those keys. The focused integration has since expanded to **18
  stock/transformed nodes**; its new transitive, nested-method-only closure
  independently passes on stock and first genuinely fails on SOAC. The earlier
  probe returns stock `(False, ('x',), ('x',))` versus transformed
  `(True, 'outer', 'outer')`; existing semantic-scope-aware descendant capture
  analysis then restores the complete strengthened matrix to
  **GREEN `18 / 18`**. A second, independent real
  production-path structured watcher regression first genuinely fails
  **`0 / 1`** at `profile snapshot must retain the preseeded owner type`; the
  sixth-file existing-registry replay then makes that same real watcher
  regression **GREEN `1 / 1`**, including actual `alpha=0` and `zeta=1`
  indices. The initial compatibility candidate remains historically
  **REJECTED as-is**. Replay initially restores **two of four** existing
  owner-field suites; never-instantiated base evidence and an invalid
  `Box.payload` index block the other two. A lambda-only closure regression
  also first genuinely fails on SOAC. Real semantic lambda-scope capture,
  exact-owner activity filtering, stock-correct fixture indices, and a truly
  unseeded non-`self` constructor then restore the combined focused gate to
  **GREEN `24 / 24`**: **20 stock/SOAC class cases plus all four existing
  owner-field suites**. The independently hardened actual watcher also passes
  **GREEN `1 / 1`**, including a raising custom metaclass, exact-owner
  instantiation, and unused-owner exclusion. After correcting an existing
  lowering test's incidental generated-name assumption, the complete serial
  Rust crate suites also pass: **lowering `372 / 372`**, **JIT `580 / 580`**,
  **optimizer `214 / 214`**, and **typed IR `54 / 54`**. The first broader
  transformed selection genuinely passes **`54 / 55`**: its remaining
  uniform-polymorphic fixture incorrectly assumes `MixedRight.mixed` follows
  `padding`, although CPython sorts `mixed` first. Renaming the unused fixture
  field to `_padding` restores the intended unequal split-key indices without
  weakening the existing assertion; the complete broader selection then passes
  **GREEN `55 / 55` in `99.38 s`**. Combined lowering/JIT Cargo test-target
  checking and package-scoped lowering/JIT Rust format checks also PASS.
  Release smoke and normally sampled fixed-eight comparison both complete
  **8 / 8**. A first repeated candidate is correctly rejected against its old
  baseline after a VM reboot changes Linux **6.8.0-137 to 6.8.0-138**; its
  third round is also independently contaminated. A fresh same-kernel
  three-round comparison subsequently completes **4 / 4**: stock score
  **`0.5401772590486644x`** versus **`0.5564929785348224x`** for the
  freshly remeasured parent, official previous-SOAC **`0.9744429326747311x`**.
  The real stock-paired `richards` regression is
  **`0.940916x`** (**95% interval `0.918484-0.965860`**) in all three
  rounds: corrected CPython sorting makes formerly uniform profiled owner
  indices mixed, and an existing planner deliberately rejects that shape.
  This required correctness repair is not performance-neutral. Full
  restored mixed-index specialization and the authoritative full
  correctness gate subsequently pass **1,259 transformed nodeids / 101
  isolated batches / 8 workers / 0 failures**. Full 97-variant acceptance
  remains unmet.

## Hypothesis and evidence

- General-purpose opportunity: faithfully preserve CPython's compiler-generated
  class `__static_attributes__` tuple and the resulting shared instance-dict
  layout. Every transformed user class is potentially affected. Correct layout
  is also required before evaluating instance-field specializations or a
  separately tracked first-bucket method-shadow proof.
- Installed pyperformance source census: **23 of 73 benchmark source
  directories contain 110 classes and approximately 514 literal-`self`
  attribute-store sites**. The fixed-four workload source contains three such
  `chaos` classes, one `comprehensions` class, eight `deltablue` classes, and
  eight `richards` classes; `bm_2to3` alone contains 31 affected classes.
  Source presence demonstrates possible compatibility/layout scope, not
  runtime hotness or a measured throughput improvement.
- Supporting evidence: `vendor/cpython/Python/compile.c:205` records an
  attribute only when its AST expression has `Store` context and its receiver is
  literally the name `self`. The compiler searches enclosing compilation scopes
  for the nearest class, deduplicates the names, and returns a sorted tuple.
  The live `c_stack` excludes the *current* compilation unit: a direct
  `self.field` store in a class's own body does not enter that same class's
  tuple, while a direct store in a nested class body is attributed to its
  enclosing outer class. Functions, lambdas, and comprehension scopes
  contribute to the nearest class already on the compiler stack; nested class
  methods contribute to the nested class. A different receiver name, an
  attribute load,
  deletion, bare annotation, or augmented assignment does not qualify:
  `vendor/cpython/Python/codegen.c:5393` emits augmented attribute stores through
  a separate path that never calls the static-attribute collector, and
  `codegen_annassign` visits its target only when an assigned value exists.
  Vendored regressions at
  `vendor/cpython/Lib/test/test_compile.py:2506` cover deduplication, nested
  functions, nested classes, and independent subclasses.
- CPython's `vendor/cpython/Python/codegen.c:1593` emits a final
  **unconditional semantic name STORE** for the computed tuple, but
  `codegen_nameop` selects its destination from the original symbol table.
  Ordinary classes store into their prepared namespace, replacing an explicit
  `__static_attributes__ = ()`; instrumented `__prepare__` mappings observe
  this final write after annotation helpers and before metaclass invocation.
  A free/captured or nonlocal binding instead writes the enclosing cell, and a
  global binding writes the module global; in those cases the class namespace
  can legitimately have **no** `__static_attributes__` entry at all. Treating
  the final STORE as an unconditional namespace assignment is incorrect.
  See `vendor/cpython/Python/codegen.c:3222` and
  `vendor/cpython/Lib/test/test_metaclass.py:153`.
- Baseline SOAC lowering passed a hard-coded empty tuple in
  `crates/soac_lowering/src/passes/ast_to_ast/rewrite_class_def/mod.rs:152`.
  `soac_py/src/soac/runtime.py:523` also inserted a fallback tuple whenever the
  prepared class namespace lacks `__static_attributes__`. These are three
  independent stock-visible failures: missing sorted lexical attributes,
  incorrect preservation of an explicit class-body override, and fabricated
  namespace entries when CPython's final STORE targets a captured cell or
  global instead.
- `vendor/cpython/Objects/dictobject.c:6702` reads the completed class's tuple
  when allocating `ht_cached_keys`, inserting eligible exact-Unicode names into
  the split-key table. Correcting the tuple can therefore change constructor
  first-insert behavior, profiled field indices, inherited-owner layouts,
  native specialization selection, and first-bucket method-absence probes.
  A subclass without its own qualifying stores still receives an empty tuple;
  inherited fields do not automatically preseed that subclass's shared keys.
- Eighteen historical explicit `__static_attributes__ = ()` declarations across
  `tests/test_late_bound_owner_fields.py`,
  `tests/test_late_owner_nonself_fields.py`,
  `tests/test_inherited_owner_fields.py`, and
  `tests/test_late_owner_scalar_regions.py` depend on a premise that does not
  hold for stock CPython. In particular, existing hard-coded split-key indices
  in the non-self and inherited-owner fixtures must be reevaluated against real
  stock layouts. A genuinely unseeded fixture must avoid lexical `self.field`
  stores, for example by assigning through a differently named receiver or
  `setattr`; merely assigning `__static_attributes__ = ()` is insufficient.
  All 18 misleading declarations are now removed. The genuine
  `UnseededRecord` control uses
  `def __init__(instance, first, middle, mark)` and `instance.field = value`,
  and explicitly asserts its compiler-produced static tuple is empty.
- Specific predicted fixture changes: `Box` acquires sorted keys
  `("cold", "marker", "payload")`, moving its asserted `payload` index from
  **1 to 2**. `StateBase` acquires
  `("packet_pending", "task_holding", "task_waiting")`, changing its existing
  `(packet_pending, task_waiting, task_holding)` index assertion from
  **`(0, 1, 2)` to `(0, 2, 1)`**. `UnseededRecord` becomes seeded with
  `("first", "mark", "middle")` unless its receiver spelling is changed.
  `WorkState` acquires `("count", "marker")`, reversing its prior insertion
  order. `DeltaLeft` and the four `StateAlpha`/`StateBeta`/`StateGamma`/
  `StateDelta` subclasses have no own qualifying stores and must remain
  unseeded; their existing inherited-constructor controls should be preserved.
  The separate uniform-polymorphic fixture's prior `MixedRight.padding` and
  `MixedRight.mixed` stores produce sorted keys `("mixed", "padding")`,
  collapsing its intended unequal-index control; the unused fixture-only
  spelling `_padding` instead yields `("_padding", "mixed")`, preserving
  the existing `MixedLeft.mixed == 0` / `MixedRight.mixed == 1` assertion.
- Expected effect: stock-visible class tuples, override behavior, namespace
  write events, and shared-key layouts become equivalent; candidate throughput,
  generated-code size, and specialization-hit changes remain unknown until
  independently measured.

## Implementation and compatibility

- Proposed production shape: collect enclosing-class lexical `self.<name>` stores
  from the original parsed AST **before** private-name mangling, annotated
  assignment rewriting, synthesized annotation helpers, or nested-class
  lowering. Record the sorted, deduplicated raw names in an explicit existing
  lowering-`Context` sidecar keyed by each original class's stable `TextRange`.
  Pass the existing original class semantic scope into class lowering and,
  after annotation synthesis and type-parameter cleanup, emit one final STORE
  to its semantically resolved destination: prepared namespace for ordinary
  class-local binding, enclosing closure cell for free/nonlocal binding, or
  module global for explicit-global binding. Remove the runtime's unsound
  missing-key fallback so a class namespace remains absent when the final
  STORE legitimately targets a cell or global. This avoids adding an artificial
  source-level binding, mutable runtime state, public API, helper, or marker.
  The saved initial compatibility implementation used five existing
  production-behavior files:
  `crates/soac_lowering/src/driver.rs`,
  `crates/soac_lowering/src/passes/ast_to_ast/context.rs`,
  `crates/soac_lowering/src/passes/ast_to_ast/rewrite_class_def/class_body.rs`,
  `crates/soac_lowering/src/passes/ast_to_ast/rewrite_class_def/mod.rs`, and
  `soac_py/src/soac/runtime.py`. The saved expanded candidate adds the existing
  `crates/soac_jit/src/module_type.rs` as its **sixth production file** for
  real preseeded-key profile replay. It reads the actual split-key entries and
  indices after watcher registration, stores weak owners and recorded entries
  in the existing profile registry, filters replay by the requested module,
  deduplicates replay against real watcher events, prunes dead owners without
  dropping Python references under the registry lock, and handles reused type
  addresses. Its genuine pre-implementation structured watcher RED now passes
  **GREEN `1 / 1`**. Existing class-body semantic child-scope traversal also
  now distinguishes genuine transitive outer-cell captures from unread outer
  bindings, restoring the strengthened semantic matrix to
  **GREEN `18 / 18`**. A subsequently genuine lambda-only descendant capture
  regression expands the class selection to **20 nodes**; the saved candidate
  now consults its existing preserved semantic lambda scope and turns the
  genuine lambda-only RED into GREEN. Initial replay restored two existing
  owner suites before the exact-owner activity filter and stock-correct
  fixture indices restored all four.
  The actual structured watcher, strengthened with a raising custom
  metaclass, a real exact instance, and an uninstantiated-owner exclusion,
  now independently passes **GREEN `1 / 1`**. The combined class/owner-suite
  focused gate passes **GREEN `24 / 24`**. Full serial lowering, JIT,
  optimizer, and typed-IR crate suites subsequently pass **`372 / 372`**,
  **`580 / 580`**, **`214 / 214`**, and **`54 / 54`**, respectively. The
  associated structured lowering regression uses the
  crate's existing test-only helper/module boundary. Package-scoped Rust formatting
  completed before the focused transformed-runtime rerun.
  Although the compatibility implementation passes all strengthened focused
  regressions, it cannot be retained before broader correctness and
  stock/previous-SOAC performance validation.
- Rejected candidate 1: injecting a synthetic source-level
  `__static_attributes__ = (...)` assignment into the original class AST looks
  smaller but changes the semantic symbol table, class-name binding decisions,
  and closure/name lookup. CPython emits its implicit final semantic STORE
  without adding a source-level symbol-table binding. Reject this candidate
  before production implementation.
- Rejected candidate 2: always appending
  `_dp_class_ns["__static_attributes__"] = (...)`, or retaining the runtime's
  existing missing-key fallback, invents a namespace attribute when stock
  CPython updates a free/nonlocal cell or global instead. The real strengthened
  stock closure control returns
  `("from enclosing scope", False, ("inferred",))`: its captured value remains
  visible during the class body, the resulting class has no static-attribute
  member, and the enclosing cell changes to the inferred tuple. Reject both
  unconditional namespace emission and runtime backfill.
- Additional independently genuine semantic RED: when the relevant enclosing
  `__static_attributes__` cell is captured only transitively by a nested method,
  stock CPython still directs the class compiler's final STORE to that cell.
  The verified stock result is **`(False, ('x',), ('x',))`**: the class has no
  static-attribute member and both the enclosing binding and nested observer
  see the inferred tuple. The current compatibility candidate instead returns
  **`(True, 'outer', 'outer')`**, incorrectly creates a class namespace member,
  and leaves both cell observations unchanged. The first eight stock/SOAC
  regression pairs remain genuinely GREEN but do not cover this transitive
  free-variable shape. The new real
  `test_static_attributes_compiler_tail_updates_cell_captured_only_by_nested_method`
  has now been added, expanding the parametrized integration selection from
  **16 to 18 nodes**. Its stock control passes and its transformed candidate
  first genuinely fails; the checked fixture expects
  `(False, ("inferred",), ("inferred",))`. Traversing actual existing semantic
  child scopes then restores the correct captured-cell final STORE, turning
  that transformed RED into GREEN. The complete strengthened
  stock/transformed selection subsequently passes **GREEN `18 / 18`**.
- Additional lambda-only closure boundary: a descendant that captures
  `__static_attributes__` solely through a lambda also makes the CPython class
  compiler's final STORE target the outer cell. An independent stock probe
  returns **`(False, ('x',), ('x',))`**, while the current candidate returns
  **`(True, 'outer', 'outer')`**. The method/class capture matrix is genuinely
  GREEN `18 / 18` but did not exercise this lambda-only child scope. The new
  real
  `test_static_attributes_compiler_tail_updates_cell_captured_only_by_lambda`
  subsequently passes on stock and genuinely fails on SOAC, expanding the
  semantic selection to **20 nodes**. The saved candidate now consults the
  existing `SemanticAstState::lambda_scope` rather than inferring a new
  binding, turning the actual transformed RED into GREEN. The complete class
  selection passes **`20 / 20`** within the combined **GREEN `24 / 24`**
  focused gate.
- CPython-visible behavior: preserve the lexical literal-`self` rule, sorted
  deterministic deduplication, nested-function inclusion, nested-class
  separation, current-class-body exclusion, nested-class-body attribution to
  the outer class, unrelated receiver exclusion, and semantic name STORE
  destination. For ordinary classes, replace an explicit class-body value and
  preserve custom prepared-mapping write visibility after annotation helpers
  and before metaclass invocation; do not add a second visible write after the
  namespace helper completes. For free/nonlocal/global targets, update the
  existing binding and preserve absence from the class namespace.
- Additional review boundary: private-name and annotation rewriting currently
  precede class lowering in `crates/soac_lowering/src/driver.rs:132`. CPython
  adds the original attribute spelling before its later opcode-name mangling;
  the focused stock control now independently confirms raw `"__private"`
  instead of the mangled runtime field name. Class-body direct stores,
  nested-class boundaries, augmented assignments, and bare annotations must
  follow actual CPython compilation scopes and code paths rather than an
  unrestricted post-rewrite AST walk. The annotation pass can append
  `__annotate_func__` after earlier class-body statements; emitting the final
  semantic STORE only after annotation synthesis and type-parameter cleanup is
  necessary for mapping-observable ordering. Existing semantic scope, not a
  newly introduced AST binding, determines cell/global/namespace destination.
- Mutable assumptions and guard lifetime: this is a compile-time source fact,
  not runtime speculation. Existing indexed-field owner/version guards,
  split-key identity checks, class mutation handling, descriptor precedence,
  dictionary promotion, ownership, pending work, and fallback remain in force.
- Actual specialization compatibility failure: CPython's class construction
  inserts the inferred static-attribute names into `ht_cached_keys` before
  SOAC's post-create `_PyDict_WatchSplitKeysForType` registration in
  `crates/soac_jit/src/module_type.rs`. The actual installed watcher does not
  replay keys that already existed at registration. The actual linked
  `_PyDict_GetKeyLayoutEvents()` also returns a **fresh independent list** on
  each call; appending synthesized rows to one returned list cannot publish
  them to a later snapshot and is not a valid replay fix. Consequently owner/key
  profile evidence disappears for `Record`, `Packet`, `WorkState`, `Box`, and
  `StateBase`, and all four existing late-bound-owner, non-self-owner,
  inherited-owner, and owner-scalar-region regression suites authentically
  fail on the original five-file candidate. CPython-correct preseeding must
  not be removed or hidden to recover the optimization. The approved sixth
  production file now implements semantics-preserving initial-key publication
  through weak owners in the existing profile registry, module-scoped replay,
  and deduplication against later watcher events. Its independent real-source
  structured test
  `watched_preseeded_split_keys_are_present_in_profile_snapshot` first
  genuinely fails **`0 / 1`** at **`profile snapshot must retain the preseeded
  owner type`**. The isolated child process enables actual Profile mode,
  compiles a
  stock CPython class whose source stores `self.zeta` and `self.alpha`, calls
  the real production `watch_split_keys_for_type`, and snapshots the actual
  linked watcher-event stream. Its expected typed result is the real owner
  identity plus `[('alpha', 0), ('zeta', 1)]`; the pre-replay path omits the
  owner completely before reaching that index assertion. The same actual
  production-path test subsequently turns **GREEN `1 / 1`**, including both
  expected sorted names and actual split-key indices. Actual focused reruns
  first restore **2 / 4** existing suites:
  `tests/test_late_bound_owner_fields.py` and
  `tests/test_late_owner_scalar_regions.py` PASS. The inherited-owner suite
  initially still FAILS because replay wrongly includes never-instantiated
  `DeltaBase` and `StateRoot`; the non-self-owner suite initially still FAILS
  because the real
  CPython-sorted `Box.payload` index is **2**, while its old fixture asserts
  **1**. In the actual pinned split-key header, key insertion preserves
  `dk_usable + dk_nentries`, while allocating an exact inline-values instance
  decreases that sum when at least two usable entries remain. The saved
  candidate now filters by real exact-owner activity and expected cached-key
  identity and corrects `Box.payload` to **2** and `StateBase`'s
  `(packet_pending, task_waiting, task_holding)` indices to **`(0, 2, 1)`**.
  The actual structured watcher is strengthened to use a custom metaclass
  that raises on metadata attribute callbacks, instantiate the exact `Point`
  owner, and exclude the separately watched `Uninstantiated` class; it passes
  **GREEN `1 / 1`**. Owners with **29 or more preseeded keys** may not
  decrement `dk_usable` on instance allocation and therefore conservatively
  lose this specialization rather than emitting unsound owner evidence.
  The exact-owner filter and CPython-correct fixture indices subsequently
  restore both remaining owner suites. The combined 20-class-node/four-owner
  focused gate passes **GREEN `24 / 24`**. The complete serial lowering, JIT,
  optimizer, and typed-IR crate suites also pass, and the broader selected
  transformed regression gate passes **`55 / 55` in `99.38 s`** after its
  unequal-index fixture is corrected. Combined lowering/JIT Cargo test-target
  checking and scoped Rust format checks also pass. Release smoke,
  fixed-eight normal sampling, and a clean same-kernel three-round
  fixed-four comparison have since completed; they expose a real
  corrected-layout `richards` regression. Full transformed pytest, the full
  correctness gate, mixed-index recovery, and full-suite acceptance remain
  PENDING.
- Guard miss or unsupported shape: no method-lookup fast path is included in
  this strategy. Existing generic CPython attribute operations remain
  authoritative; no owner cache, `FunctionEnv` ABI change, or new runtime helper
  is proposed. The existing class-creation runtime must stop inventing missing
  namespace entries after the semantically correct compiler tail.
- Focused regression coverage: new real
  `tests/test_class_static_attributes.py` initially executes eight identical cases under
  stock and transformed module loaders. Before production changes, stock passes
  **8 / 8 in `0.04 s`**; frozen unchanged transformed production fails
  **8 / 8 in `0.77 s`**. Earlier genuine checkpoints were stock
  **5 / 5 in `0.04 s`** / transformed **5 / 5 failures in `0.68 s`**, then
  stock **7 / 7 in `0.05 s`** / transformed **7 / 7 failures in `0.76 s`**.
  Coverage includes lexical store-shape exclusions, raw private spelling,
  annotated stores, loop/comprehension targets, literal `self` inside a static
  method, nested functions/lambdas/classes, explicit class-body override,
  custom prepared-mapping write events, actual annotation-helper-before-static
  ordering, metaclass ordering, independent inherited-owner tuples,
  current-class-body exclusion/nested-class-body outer attribution, and
  captured-cell mutation with legitimate class-namespace absence. The final
  eighth case verifies the complete explicit-global, explicit-nonlocal,
  explicit-local, and unread-enclosing-binding matrix: global/nonlocal stores
  update their external bindings and leave no class attribute; an unread outer
  name remains unchanged while the class gets its own tuple; an explicit local
  class binding is replaced without mutating the outer name. The actual saved
  compatibility candidate initially passed the then-complete parametrized
  module: **GREEN `16 / 16`**, consisting of **eight stock controls and all
  eight previously RED transformed cases**. A subsequently independent live
  stock/transformed probe identified the missing ninth transitive
  nested-method closure shape. The corresponding real paired regression is
  now present, expanding the selection to **18 nodes**; the new stock control
  is GREEN and its transformed candidate first genuinely fails. Existing
  semantic child-scope capture analysis then turns that exact transformed RED
  into GREEN, and the complete strengthened stock/transformed selection
  passes **GREEN `18 / 18`**. A tenth independently genuine lambda-only
  transformed RED is then corrected through the existing semantic lambda
  scope, yielding **GREEN `20 / 20`** for the class suite and
  **GREEN `24 / 24`** with the four existing owner-field suites. Preserve the
  earlier `16 / 16`, `18 / 18`, and intervening genuine REDs as historical
  checkpoints.
- Independent structured lowering regression:
  `crates/soac_lowering/src/test.rs` test
  `class_namespace_helper_finishes_with_original_sorted_static_attribute_store`
  executes real `lower_python_to_blockpy_for_testing`, reads the production
  tracked `ast-to-ast` result, finds the actual generated
  `_dp_class_ns_Subject` function, and structurally inspects its final
  assignment target and tuple literals. Untouched production genuinely
  returns **`None` instead of `Some(["__private", "alpha", "zeta"])`**.
  This asserts real AST structure, unmangled original spelling, deterministic
  sorting, annotation-helper ordering, and final namespace subscript emission;
  it does not inspect rendered text. An initial Rust-2021 `let`-chain
  compilation issue was a test-fixture/toolchain mistake, **not** the genuine
  RED; it was corrected before recording the actual missing-tail assertion.
  The same production-path structured regression subsequently turns
  **GREEN `1 / 1`** against the saved candidate. Existing owner-field controls
  subsequently expose the independently genuine watcher/preseeding failure;
  the sixth-file replay, exact-owner filtering, and corrected stock split-key
  assertions later restore all four suites within the focused **`24 / 24`**
  gate.
- Independent structured watcher regression:
  `crates/soac_jit/src/module_type.rs` test
  `watched_preseeded_split_keys_are_present_in_profile_snapshot` executes in
  an isolated Profile-mode child, creates an unmodified stock CPython class
  with preseeded `alpha`/`zeta` keys, invokes the actual production
  `watch_split_keys_for_type`, and reads the actual watcher snapshot and
  stable-owner type table. The unmodified watcher path genuinely fails
  **`0 / 1`** at **`profile snapshot must retain the preseeded owner type`**;
  its subsequent typed expectation is `[('alpha', 0), ('zeta', 1)]`. After
  implementing the existing-registry weak-owner replay in the sixth production
  file, this exact isolated-child, actual-watcher, typed-layout regression
  turns **GREEN `1 / 1`**. The same actual production regression is then
  strengthened with a metaclass that raises on `__module__`/`__qualname__`
  interception, explicit exact-owner instance activation, and a separate
  never-instantiated owner's required absence; this strengthened version also
  passes **GREEN `1 / 1`**. This RED-to-GREEN proof is independent of the
  already-GREEN structured lowering test and does not assert on rendered text
  or synthetic watcher lists. All four end-to-end owner-field suites now pass
  within the focused **`24 / 24`** gate, and the full serial JIT suite passes
  **`580 / 580`**.

## Benchmark protocol and coverage

- Fixed benchmark selection: exploratory fixed eight
  `chaos,comprehensions,deltablue,fannkuch,float,nbody,richards,spectral_norm`;
  repeated targeted four `chaos,comprehensions,deltablue,richards`. The actual
  default pyperformance acceptance suite contains 97 benchmark variants; all
  previously retained comparison artifacts covered at most eight. No existing
  full-suite completion, failure inventory, or acceptance score is available.
- Comparison command and rounds: fixed-eight debug-single-value release smoke
  and normally sampled single-round comparisons; clean same-kernel fixed-four
  stock/SOAC comparisons each use three independently started,
  order-alternating rounds. A prior three-round candidate after a VM restart
  is explicitly rejected because its previous baseline has incompatible
  kernel metadata and its third candidate round is contaminated. One-round
  full-suite readiness and three-round full-suite acceptance remain PENDING.
- Baseline revision or artifact: integrated `srxzvruu d0655b62`;
  `work/pyperformance/comparison-20260819-231117-FXavZ9/summary.json` for
  the normal fixed eight and fresh same-kernel
  `work/pyperformance/comparison-20260820-090642-20GRdt/summary.json` for
  the repeated fixed four.
- Candidate revision or artifact: working `lwyqsqsm`; release smoke
  `work/pyperformance/comparison-20260820-003945-3Z8Vlo/summary.json`,
  normal fixed eight
  `work/pyperformance/comparison-20260820-004139-Oqxfrb/summary.json`, and
  clean same-kernel repeated fixed four
  `work/pyperformance/comparison-20260820-091112-iaT71z/summary.json`.
  `work/pyperformance/comparison-20260820-085800-o51U25` is preserved as a
  rejected cross-kernel / unstable-round attempt, not headline evidence.
- Profile evidence: baseline artifacts independently generated under their
  then-existing incorrect class-layout behavior; fresh candidate Profile and
  Apply evidence is required because seeded layouts and field indices may
  change. Reusing baseline profile evidence would invalidate the comparison.
- Module selection: both baseline and candidate transform benchmark
  `__main__` plus compiler-owned `soac.runtime`; no standard-library or
  third-party dependency module is transformed. Full-suite third-party
  transformation and dependency availability remain unverified.
- Completed/failed benchmarks: baseline and candidate fixed-eight smoke and
  normal comparisons each complete **8 / 8**; fresh same-kernel repeated
  controls each complete **4 / 4** across three rounds and **120 actual
  candidate Apply worker PIDs**, with zero worker errors. The earlier
  restarted-VM comparison completes execution but correctly fails its
  previous-baseline compatibility check and is not valid performance
  evidence. The other **89 / 97** acceptance variants remain untested.
- Transformed benchmark/dependency modules: both revisions transform project
  `__main__` and `soac.runtime`; no third-party dependency module is
  transformed.
- Transformed standard-library modules: none on either revision.
- Compiled functions or hot-path coverage: baseline fixed-eight recorded
  `chaos` 32, `comprehensions` 19, `deltablue` 76, `fannkuch` 1, `float` 7,
  `nbody` 6, `richards` 51, and `spectral_norm` 7 distinct compiled functions;
  candidate function coverage is identical. Normal comparisons include
  **80** actual measured Apply worker PIDs; repeated fixed-four comparisons
  include **120**. Summary `worker_count` counts stable work directories,
  not the ten independently started measured PIDs per benchmark and round.
- Separate full-suite workflow preflight: the shared pyperformance benchmark
  venv contains only six installed distributions, nine previously prepared
  benchmark names, and no populated pip cache. Its 97-variant acceptance
  manifest selects 68 active source directories; 21 of those directories
  account for 26 active benchmark variants and require 34 distinct pinned
  direct packages plus transitives. The broader installed-source census is 23
  dependency directories and 36 package names because `hg_startup` and `yaml`
  are installed but absent from the active acceptance manifest.
  The current recipe's common `--inherit-environ` list strips the existing
  internal `PIP_INDEX_URL` and proxy variables from benchmark-venv pip in
  both stock and default SOAC modes; the existing extra-inheritance override
  applies only to SOAC. A separate credential-safe common index/proxy
  inheritance workflow fix and a complete dependency-failure inventory are
  required before a meaningful 97-variant acceptance run. No secret values
  were logged, and no full-suite smoke has been run.

## Measurements

| Metric | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| Fixed-eight stock geometric score | `0.6865833897338185x` | `0.6732463358425108x` | official previous SOAC `1.0106273904686902x`; paired robust `0.98687449x` |
| Fixed-eight previous-SOAC geometric score | `0.9970497902989457x` against its earlier retained parent | `1.0106273904686902x` against `srxzvruu` | single-round mean confounds stock/environment drift |
| Clean same-kernel targeted-four stock geometric score, three rounds | `0.5564929785348224x` | `0.5401772590486644x` | previous SOAC `0.9744429326747311x`; paired robust `0.974718x` |
| `chaos` stock / SOAC, fixed eight | `30.815584 ms / 40.836245 ms`; `0.7546135673x` | `30.034195 ms / 40.722138 ms`; `0.7375397547x` | previous SOAC `1.0028020889x` |
| `comprehensions` stock / SOAC, fixed eight | `8.353157 us / 44.127205 us`; `0.1892972073x` | `7.729233 us / 44.048744 us`; `0.1754700039x` | previous SOAC `1.0017812226x` |
| `deltablue` stock / SOAC, fixed eight | `1.493638 ms / 2.128185 ms`; `0.7018367767x` | `1.444428 ms / 2.187004 ms`; `0.6604599741x` | previous SOAC `0.9731052465x`; not reproduced in clean repeated rounds |
| `fannkuch` stock / SOAC, fixed eight | `193.108025 ms / 252.486010 ms`; `0.7648266365x` | `183.006612 ms / 248.257510 ms`; `0.7371644541x` | previous SOAC `1.0170327143x` |
| `float` stock / SOAC, fixed eight | `36.420081 ms / 36.104220 ms`; `1.0087485972x` | `35.963796 ms / 35.328936 ms`; `1.0179699595x` | previous SOAC `1.0219447183x` |
| `nbody` stock / SOAC, fixed eight | `48.906544 ms / 65.564992 ms`; `0.7459246531x` | `48.152972 ms / 61.451408 ms`; `0.7835942909x` | previous SOAC `1.0669404423x`; unchanged generated code |
| `richards` stock / SOAC, fixed eight | `22.575522 ms / 21.748692 ms`; `1.0380174613x` | `22.539122 ms / 22.854688 ms`; `0.9861925001x` | previous SOAC `0.9516074610x`; repeated mixed-index regression confirmed |
| `spectral_norm` stock / SOAC, fixed eight | `49.919552 ms / 60.544521 ms`; `0.8245098220x` | `48.871927 ms / 57.392428 ms`; `0.8515396169x` | previous SOAC `1.0549217411x`; unchanged generated code |
| Full 97-benchmark geometric stock score | unavailable | PENDING | PENDING |
| Optimized typed-IR blocks / functions, fixed eight | `2,866 / 204` | `2,866 / 204` | unchanged |
| Optimized typed-IR blocks / functions, repeated four | `2,265 / 183` | `2,265 / 183` | unchanged |
| Pre-optimization BlockPy bytes, fixed eight | `14,398,752` | `14,543,856` | `+145,104` / `+1.008%` |
| Apply-mode native code bytes / machine blocks, fixed eight | `23,952,600 / 1,570,300` | `23,890,880 / 1,566,600` | `-61,720 bytes / -3,700 blocks`; hidden `381,080` unchanged |
| Actual Apply native bytes / machine blocks, all three fixed-four rounds | `56,982,240 / 3,727,500` | `56,797,080 / 3,716,400` | `-185,160 bytes / -11,100 blocks`; hidden `777,240` unchanged |

## Attempt history

### Attempt 1: establish CPython parity before method-lookup speculation

- Selected at 2026-08-19 23:50 PDT on integrated
  `srxzvruu d0655b62`, working change `lwyqsqsm`.
- Change: inspected pinned CPython compiler, class-body code generation,
  metaclass mapping regressions, split-key initialization, existing SOAC class
  lowering/runtime, affected field fixtures, installed benchmark sources, and
  retained comparison summaries; added and strengthened eight
  stock/transformed production-path compatibility regressions. Rejected both
  source-level synthetic assignment and unconditional namespace store/runtime
  backfill because they change symbol-table, captured-cell, or class-namespace
  semantics. Selected the stable-`TextRange`, existing-`Context` sidecar,
  existing class semantic scope, destination-aware final STORE, and removal of
  runtime backfill. Added an independent structured actual-lowering regression;
  production implementation has not started.
- Measurements and coverage: complete fixed eight `0.6865833897338185x` stock,
  complete three-round targeted four `0.5613133486246105x` stock; no previous
  comparison exceeded eight of 97 acceptance variants. Candidate results,
  generated-code deltas, and full-suite readiness are PENDING.
- Compatibility and tests: initial unchanged stock **GREEN `5 / 5` in
  `0.04 s`**, initial unchanged transformed **RED `5 / 5` in `0.68 s`**;
  strengthened unchanged stock **GREEN `7 / 7` in `0.05 s`**, strengthened
  unchanged transformed **RED `7 / 7` in `0.76 s`**; final frozen unchanged
  stock **GREEN `8 / 8` in `0.04 s`**, final frozen unchanged transformed
  **RED `8 / 8` in `0.77 s`**. The precise mismatches
  are: (1) an empty transformed tuple instead of the stock sorted seven-name
  tuple including raw `"__private"`; (2) missing separate outer and nested
  class tuples; (3) preserved `("manually_chosen",)` instead of computed
  `("inferred",)` or `()`; (4) missing final prepared-mapping
  `("static-write", ("inferred",))` before metaclass creation, leaving the
  wrong manually supplied tuple, now also proving its required position after
  the annotation-helper write; (5) missing base/own-field tuples while
  subclasses with no own stores must still stay empty; (6) incorrect
  current-class-body exclusion and nested-class-body outer attribution;
  (7) failure to update the captured enclosing cell while preserving legitimate
  absence of the class attribute; and (8) failure to honor explicit global,
  explicit nonlocal, explicit local, and unread-enclosing semantic binding
  destinations, including proper class-attribute absence and preservation of
  unrelated outer values. Independently, the real production
  `lower_python_to_blockpy_for_testing` structured regression
  `class_namespace_helper_finishes_with_original_sorted_static_attribute_store`
  fails with actual `None` versus expected
  `Some(["__private", "alpha", "zeta"])`; an earlier Rust-2021 `let`-chain
  compile failure was corrected and is explicitly **not** counted as a genuine
  production RED. Existing fixture corrections, candidate structured/semantic
  GREEN, benchmarks, and the full correctness gate are PENDING. Eighteen
  explicit-empty fixture declarations are historically based on invalid stock
  semantics.
- Result: IN PROGRESS; no throughput, compatibility fix, or acceptance claim.
- Reason: method-absence and split-index conclusions would otherwise be drawn
  from an incorrect emulation of CPython's class-created key table. The
  smallest apparent AST-injection shortcut is itself semantically unsound.

### Attempt 2: restore class semantics and expose preseeded-key watcher loss

- Change: implemented original-AST lexical collection before name/annotation
  rewrites, the stable-source-range lowering-`Context` sidecar, existing
  semantic-scope-aware final class-body STORE, and removal of fabricated
  runtime namespace backfill across the five previously approved production
  files. Kept the regression probe within the existing test-only helper/module
  boundary and ran package-scoped Rust formatting before focused transformed
  validation.
- Compatibility and tests: the genuine production structured-lowering
  assertion changed from actual `None` versus
  `Some(["__private", "alpha", "zeta"])` to **GREEN `1 / 1`**. The complete
  focused stock/transformed semantic matrix is **GREEN `16 / 16`**, including
  all eight independently observed transformed RED-to-GREEN transitions and
  the global/nonlocal/local/unread closure controls. A subsequent independent
  transitive nested-method-only closure probe then discovers an additional
  real semantic RED: stock **`(False, ('x',), ('x',))`** versus transformed
  candidate **`(True, 'outer', 'outer')`**. The new focused regression has now
  expanded the stock/transformed selection to **18 nodes**; its stock control
  genuinely passes and its transformed candidate genuinely fails. The saved
  test expects `(False, ("inferred",), ("inferred",))`; the semantic repair
  remains PENDING. Do not misinterpret the earlier `16 / 16` as complete
  class-closure parity.
- Existing-specialization regression: all four existing suites
  `tests/test_late_bound_owner_fields.py`,
  `tests/test_late_owner_nonself_fields.py`,
  `tests/test_inherited_owner_fields.py`, and
  `tests/test_late_owner_scalar_regions.py` then **FAIL**. Correct CPython
  class creation has already inserted inferred names into its shared
  `ht_cached_keys` by the time SOAC calls `_PyDict_WatchSplitKeysForType` after
  creating the type. The actual watcher implementation does not replay those
  preexisting keys, so profiled `Record`, `Packet`, `WorkState`, `Box`, and
  `StateBase` owner fields disappear instead of becoming available with their
  corrected CPython indices. The actual `_PyDict_GetKeyLayoutEvents()` returns
  a fresh independent list, so mutating a retrieved list cannot repair the
  missing producer-side events. The approved candidate expands to a sixth
  existing production file, `crates/soac_jit/src/module_type.rs`. Its proposed
  structured
  `watched_preseeded_split_keys_are_present_in_profile_snapshot` regression now
  genuinely **fails `0 / 1`** at
  **`profile snapshot must retain the preseeded owner type`**. It uses an
  isolated Profile-mode child, an actual stock-preseeded class, the actual
  production watcher, and the actual linked watcher snapshot; the expected
  typed keys/indices are `[('alpha', 0), ('zeta', 1)]`. The watcher replay
  implementation and structured GREEN are PENDING.
- Measurements and coverage: no candidate benchmark, stock comparison,
  previous-SOAC delta, generated-code measurement, full-suite readiness run,
  or full correctness gate has been completed.
- Result: **REJECT candidate as-is** despite the original focused compatibility
  GREEN. Recovering specialization requires turning the independently genuine
  structured preseeded-key watcher RED into GREEN with a sound initial
  publication/replay design;
  recovering CPython compatibility also requires the newly demonstrated
  transitive nested-method closure to pass before any throughput or acceptance
  claim.

### Attempt 3: replay actual preseeded split keys without retaining owner types

- Change: expanded the saved implementation to its approved sixth existing
  production file, `crates/soac_jit/src/module_type.rs`. After actual CPython
  watcher registration, inspect the owner's real `ht_cached_keys` header and
  Unicode entries, store their actual names/indices alongside a weak owner in
  the already-existing profile registry, and merge live owner rows into the
  requested module's profile snapshot. Deduplicate against ordinary watcher
  events, prune collected owners, handle heap-address reuse, and drop removed
  weak references only after releasing the registry lock.
- Structured production RED-to-GREEN: the isolated Profile-mode test
  `watched_preseeded_split_keys_are_present_in_profile_snapshot` first fails
  **`0 / 1`** with
  **`profile snapshot must retain the preseeded owner type`**. Against the
  sixth-file replay implementation, the exact same real stock class, actual
  watcher, and actual profile-snapshot path then passes **GREEN `1 / 1`**,
  structurally proving `[('alpha', 0), ('zeta', 1)]`.
- Compatibility and existing-suite state: preserve the prior real lowering
  GREEN `1 / 1`, historical initial stock/SOAC GREEN `16 / 16`, independently
  observed ninth nested-method closure stock GREEN/SOAC RED, and all four
  authentic owner-field suite failures on the earlier no-replay candidate.
  The strengthened 18-node semantic matrix, fresh reruns of all four existing
  suites, corrected fixture indices, candidate benchmarks, generated-code
  sizes, full correctness gate, and 97-variant acceptance remain PENDING.
- Result: watcher-specific structured regression GREEN; the expanded candidate
  is **NOT YET RETAINABLE** until semantic and specialization regressions are
  independently restored.

### Attempt 4: restore transitive captured-cell class-tail semantics

- Change: extend the existing class-body semantic child-scope traversal to
  detect actual nonlocal captures through nested functions/classes while
  preserving explicit local/global/nonlocal bindings and distinguishing
  unread enclosing names. Pass the resulting capture fact into the existing
  semantic-destination-aware final class STORE. No seventh production file or
  artificial source-level binding is added.
- Genuine semantic RED-to-GREEN: the strengthened
  `test_static_attributes_compiler_tail_updates_cell_captured_only_by_nested_method`
  first passes on stock and fails on SOAC; the independent original probe
  returns stock `(False, ('x',), ('x',))` versus transformed
  `(True, 'outer', 'outer')`. The fixed transformed runtime now matches the
  stock expectation `(False, ("inferred",), ("inferred",))`, and the complete
  expanded stock/transformed class suite passes **GREEN `18 / 18`**.
- Preserved structured evidence: the genuine lowering RED-to-GREEN
  **`1 / 1`** and genuine actual-watcher/sorted-preseed RED-to-GREEN
  **`1 / 1`** remain independently established. Earlier failures in all four
  existing owner-field suites remain historical evidence; post-replay reruns
  are PENDING.
- Remaining boundaries: callback-free metaclass-safe replay owner metadata,
  all four existing owner-field suite reruns, corrected fixture indices,
  candidate stock/previous-SOAC benchmarks, full correctness gate, and full
  97-benchmark acceptance remain PENDING.
- Result: strengthened focused semantic matrix and both structured
  regressions GREEN; the expanded candidate is **NOT YET RETAINABLE**.

### Attempt 5: distinguish exact owner activity from class-key preseeding

- Existing-suite outcome: after sixth-file actual-layout replay, existing
  `tests/test_late_bound_owner_fields.py` and
  `tests/test_late_owner_scalar_regions.py` authentically PASS. The earlier
  all-four failure remains genuine historical evidence, while current
  recovery is only **2 / 4**.
- Remaining inherited-owner failure:
  `tests/test_inherited_owner_fields.py` observes replayed rows for `DeltaBase`
  and `StateRoot`, even though those exact types were never instantiated;
  their constructors run only against separate subclass instances. Publishing
  merely class-preseeded base layouts violates the exact-owner activity and
  ambiguity contract.
- Remaining non-self-owner failure:
  `tests/test_late_owner_nonself_fields.py` observes that CPython's sorted
  static tuple places `Box.payload` at split-key index **2**, while the
  historical fixture incorrectly asserts index **1**. The fixture must follow
  real stock layout without weakening owner-uniqueness or indexed-hit guards.
- Verified activity discriminator: inserting or preseeding a split key
  increments `dk_nentries` and decrements `dk_usable`, leaving their sum
  unchanged. Allocating a real exact-type inline-values instance decrements
  `dk_usable` without adding an entry. Comparing
  `dk_usable + dk_nentries` against initial shared-key capacity can therefore
  distinguish an instantiated exact owner from an unused preseeded base. A
  genuine structured never-instantiated-owner RED and production filtering
  remain PENDING.
- Newly uncovered closure boundary: a lambda-only descendant independently
  returns stock `(False, ('x',), ('x',))` versus transformed
  `(True, 'outer', 'outer')`, despite the method-based **`18 / 18`** matrix
  passing. Its focused regression and repair remain PENDING.
- Result: **2 / 4** existing owner suites restored; callback-free
  custom-metaclass structured validation, lambda-only closure parity, both
  remaining suites, full correctness, and performance/acceptance remain
  PENDING. No retention or performance-gain claim.

### Attempt 6: harden activity, metaclass safety, lambda scopes, and fixtures

- Genuine lambda RED: the new real stock/transformed
  `test_static_attributes_compiler_tail_updates_cell_captured_only_by_lambda`
  passes on stock and genuinely fails on the prior SOAC candidate. It expands
  the semantic selection from the historical GREEN `18 / 18` to **20 nodes**;
  saved production resolves the existing preserved semantic lambda scope and
  turns this genuine transformed RED into GREEN within the passing
  **`20 / 20`** class matrix.
- Hardened actual-watcher GREEN: the existing isolated Profile-mode
  production-path regression now uses `MetadataBlockingMeta`, which raises if
  profiling invokes `__module__` or `__qualname__` attribute hooks; it watches
  both a real `Point` and `Uninstantiated`, instantiates only `Point`, and
  requires `Point`'s actual `alpha=0`/`zeta=1` rows while excluding the
  unused exact owner. The strengthened real watcher/snapshot regression
  passes **GREEN `1 / 1`**.
- Exact-owner implementation and boundary: retain the watched cached-key
  identity and initial `dk_usable + dk_nentries`; replay only after actual
  exact inline-instance allocation decreases that value. In pinned CPython,
  classes with **29 or more preseeded keys** can start with
  `dk_usable <= 1`, so allocation does not decrease it. These classes
  conservatively miss the optimization instead of receiving speculative
  exact-owner evidence.
- Saved truthful fixtures: `Box.payload` now correctly requires split-key
  index **2**, and `StateBase` requires its sorted
  `(packet_pending, task_waiting, task_holding)` tuple **`(0, 2, 1)`**.
  All 18 misleading explicit-empty class declarations are removed. The genuine
  `UnseededRecord` fixture uses
  `def __init__(instance, first, middle, mark)` and `instance.field` stores,
  with an explicit compiler-produced empty-tuple assertion. Owner uniqueness,
  inherited-base exclusion, first-insertion fallback, and indexed-hit
  assertions are preserved.
- Validation state: the combined **24-node** selection contains the
  **20-node** expanded stock/transformed class matrix and all four existing
  owner-field suites. The complete selection now passes
  **GREEN `24 / 24`**, including the lambda RED-to-GREEN and both previously
  failing inherited/non-self owner suites. The hardened actual watcher
  independently passes **GREEN `1 / 1`**. Full correctness, benchmarks, and
  full-suite acceptance remain PENDING.
- Result: all focused class, owner-specialization, and structured watcher
  gates GREEN; candidate **NOT YET RETAINABLE** pending full correctness and
  performance validation.

### Attempt 7: remove incidental generated-name dependence from a lowering test

- Broader lowering-library discovery: existing structured-storage test
  `crates/soac_lowering/src/passes/test.rs` test
  `closure_backed_coroutine_records_explicit_storage_layout` hard-coded the
  synthesized logical name `_dp_eval_7` and storage name
  `_dp_cell__dp_eval_7`. The additional correct class-tail lowering changes
  the incidental generated-name suffix to `_dp_eval_9` without changing
  coroutine storage semantics.
- Fix: identify the actual preserved evaluation slot by its semantic
  `_dp_eval_` role/prefix, assert that its storage name is exactly
  `_dp_cell_{logical_name}`, and retain its real `ClosureInit::Deferred` and
  `PreservedSlotStorage::PyObjectOrNull` structural assertions. This corrects
  brittle test identity, not a production semantic regression.
- Validation state: the focused class/owner gate remains authentically
  **GREEN `24 / 24`**. The corrected complete lowering-library rerun now
  passes **GREEN `372 / 372`**; broad transformed pytest, the full
  correctness gate, candidate benchmarks, and 97-variant acceptance remain
  PENDING.
- Workflow lesson: structured lowering tests should assert preserved-slot
  role, storage relationship, and initialization semantics instead of
  incidental fresh-name counters shared with unrelated compiler rewrites.

### Attempt 8: validate the affected Rust crates serially

- Full lowering crate: **GREEN `372 / 372`**, including the genuine
  class-static-tail structured RED-to-GREEN and the repaired
  semantic-role/preserved-storage coroutine regression.
- Full JIT crate: **GREEN `580 / 580`**, including the actual Profile-mode,
  production-watcher snapshot regression with raising custom metaclass,
  exact-instance activation, and never-instantiated-owner exclusion.
- Full optimizer crate: **GREEN `214 / 214`**.
- Full typed-IR crate: **GREEN `54 / 54`**.
- These crate suites were run serially, avoiding shared Cargo target/package
  locks, and complement the focused stock/transformed plus owner-field
  **GREEN `24 / 24`** gate. They are not a substitute for the broad
  transformed pytest run, full `just test-all` gate, measured candidate
  stock/previous-SOAC performance, or full 97-variant acceptance; each of
  those remains PENDING.

### Attempt 9: preserve the real unequal-index polymorphic fixture control

- Broader transformed regression selection: **`54 / 55` PASS**, with one
  genuine failure in
  `tests/test_uniform_polymorphic_nonself_fields.py` test
  `test_uniform_polymorphic_nonself_fields_reuse_each_exact_owner_guard`.
- Root cause: `MixedLeft` has only `self.mixed`, while the original
  `MixedRight` stores `self.padding` and `self.mixed`. Correct CPython
  static-attribute preseeding sorts the latter as `("mixed", "padding")`,
  placing `mixed` at index **0** for both owners. The existing real-profile
  assertion intentionally requires
  `{("MixedLeft", 0), ("MixedRight", 1)}` to prove unequal-owner-index
  behavior, so the fixture's old insertion-order premise no longer creates
  the case it claims to test.
- Safe fixture correction: rename the unused `MixedRight` field from
  `padding` to `_padding`, producing stock-sorted keys
  `("_padding", "mixed")` and restoring `MixedRight.mixed` index **1**.
  Keep the existing distinct-owner/index assertion and all production
  behavior unchanged; do not weaken the specialization regression to accept
  equal indices. The corrected fixture source is saved and retains its
  original strict negative control.
- Historical result: the original broader run genuinely remains
  **`54 / 55`**, preserving the pre-correction fixture failure. The
  subsequent corrected rerun is recorded separately below.

### Attempt 10: rerun the complete broader transformed regression selection

- Corrected 55-node transformed regression selection:
  **GREEN `55 / 55` in `99.38 s`**.
- The uniform-polymorphic test still requires
  `{("MixedLeft", 0), ("MixedRight", 1)}`; the sole unused-fixture rename
  `padding` to `_padding` creates those genuinely distinct stock-sorted
  indices without weakening the existing real-profile assertion.
- This rerun also preserves the focused class/owner **GREEN `24 / 24`**,
  both genuine structured lowering/watcher RED-to-GREEN proofs, callback-free
  metaclass-safe watcher replay, exact-instance activation,
  never-instantiated-owner exclusion, corrected inherited/non-self layouts,
  genuine non-`self` unseeded fixture, and serial full-crate
  **`372 / 372`**, **`580 / 580`**, **`214 / 214`**, and **`54 / 54`**
  evidence. Classes with at least 29 preseeded keys continue to conservatively
  miss replay when their initial `dk_usable` prevents the activity signal.
- Additional actual validation: combined
  `cargo check -p soac_lowering -p soac_jit --tests` **PASS**, and
  `just fmt-rust-check soac_lowering soac_jit` **PASS**.
- Explicitly still PENDING: the complete unselected transformed pytest suite,
  full `just test-all`, release-runtime validation, candidate
  stock/previous-SOAC comparisons, and full 97-variant acceptance. The
  successful 55-node selection is not the full correctness or performance
  gate.

### Attempt 11: validate release smoke and normally sampled fixed-eight coverage

- Release debug-single-value smoke
  `comparison-20260820-003945-3Z8Vlo` completes **8 / 8** with the same
  **198 distinct source functions**, **2,866 typed blocks / 204 typed
  functions**, and zero worker failures. Its stock geometric score is
  **`0.6142843991543956x`** and nominal prior score
  **`1.0498709691001369x`**, but each benchmark has only **one measured
  worker / one value**. In particular, `comprehensions` reports a misleading
  **`1.488579x`** previous-SOAC gain with only **`0.256 ms`** measured
  after **`405.353 ms`** setup. These are release/correctness and emitted-code
  observations, **not valid performance evidence**.
- Normally sampled `comparison-20260820-004139-Oqxfrb` then completes
  **8 / 8** with **80 actual measured Apply worker PIDs**, unchanged source
  identities and typed coverage. Its stock geometric score is
  **`0.6732463358425108x`** versus the parent's
  **`0.6865833897338185x`**. The official arithmetic previous-SOAC score
  **`1.0106273904686902x`** is misleading: independently stock-paired
  robust comparison is only **`0.98687449x`**. Single-round stock-paired
  `deltablue` is **`0.948981x`** (**95% `0.8966915-0.9758088`**) and
  `richards` **`0.9695335x`** (**95% `0.9113095-1.0172322`**); repeated
  rounds are necessary to distinguish causal changes from noise.
- Fixed-eight native code changes **`23,952,600 -> 23,890,880` bytes**
  (**`-0.257676%`**) and machine blocks **`1,570,300 -> 1,566,600`**
  (**`-0.235624%`**); hidden trampoline bytes remain **`381,080`**.
  Pre-optimization BlockPy changes **`14,398,752 -> 14,543,856` bytes**
  (**`+1.007754%`**). Exact candidate compiled-function counts are
  `chaos` **32**, `comprehensions` **19**, `deltablue` **76**,
  `fannkuch` **1**, `float` **7**, `nbody` **6**, `richards` **51**,
  and `spectral_norm` **7**, unchanged from their baseline.
- Result: release behavior and actual transformed coverage pass; the
  apparent normal previous-SOAC mean is **not proof of a speedup**. The
  full correctness gate and clean repeated comparison remain necessary.

### Attempt 12: reject cross-kernel baseline and unstable restarted-VM rounds

- After the Lima VM restarts, the pinned interpreter and benchmark
  selection remain unchanged but the guest kernel changes from
  **`Linux-6.8.0-137-generic-aarch64-with-glibc2.39`** to
  **`Linux-6.8.0-138-generic-aarch64-with-glibc2.39`**.
  `comparison-20260820-085800-o51U25` executes all **three fixed-four
  rounds / 120 Apply PIDs**, but
  `scripts/summarize_pyperformance_comparison.py` correctly rejects the
  previous revision's incompatible `platform` metadata. Do not compare the
  candidate against its old-kernel baseline or invent a previous-SOAC score.
- The paired candidate-only stock score **`0.5418253227650655x`** can be
  reconstructed without a baseline, but the run is independently unstable:
  `chaos` Apply means are **`41.55 ms`**, **`42.21 ms`**, and
  **`71.39 ms`** across the three rounds, with third-round workers ranging
  from **`60.98 ms` to `92.22 ms`** while stock stays near **`30 ms`**.
  `deltablue` simultaneously rises from approximately **`2.12 ms`** to
  **`2.76 ms`**; other candidate workloads also worsen in that round.
- Preserve the artifact and negative result, but reject its timing as
  **cross-kernel / environmentally contaminated**, not an optimization
  regression or win. The existing comparison recipe validates the previous
  platform only after running all expensive rounds; it needs an early
  baseline-metadata preflight and measured-worker stability reporting.
- Result: **REJECT this comparison** and independently rerun both the
  integrated parent and candidate on the same current guest kernel.

### Attempt 13: establish clean same-kernel regression and its exact mixed-index cause

- Fresh integrated-parent comparison
  `comparison-20260820-090642-20GRdt` and candidate
  `comparison-20260820-091112-iaT71z` both run on
  **`Linux-6.8.0-138-generic-aarch64-with-glibc2.39`** with the same
  independently regenerated Profile evidence, four fixed workloads, and
  **three order-alternating rounds / 120 candidate Apply worker PIDs**.
  All workloads complete with zero worker errors. The candidate stock
  geometric score is **`0.5401772590486644x`** against parent
  **`0.5564929785348224x`**; official previous SOAC is
  **`0.9744429326747311x`**. The independent round-stratified geometric
  result is **`0.981210x` raw / `0.974718x` stock-paired**
  (**paired 95% `0.961552-0.986371`**).
- Per-workload stock / previous scores are `chaos`
  **`0.7197702230x / 0.9862534777x`**, `comprehensions`
  **`0.1756126047x / 0.9517733507x`**, `deltablue`
  **`0.6856906831x / 0.9930047726x`**, and `richards`
  **`0.9823525538x / 0.9672800189x`**.
  Robust stock-paired `chaos` **`0.982253x`**
  (**95% `0.958263-1.018941`**) and `deltablue` **`1.006679x`**
  (**95% `0.987108-1.024542`**) are neutral. `comprehensions` appears
  adverse at **`0.970176x`** (**95% `0.936171-0.983354`**), but its
  generated native bodies, profiled indices, and selected plans are exactly
  unchanged: it is an unchanged-code negative control, not a proven causal
  effect of the class-layout repair.
- `richards` genuinely regresses in **all three** paired rounds
  (**`0.940998x`**, **`0.935934x`**, **`0.945843x`**): raw
  **`0.958229x`** (**95% `0.939852-0.979582`**), stock-paired
  **`0.940916x`** (**95% `0.918484-0.965860`**). This is not noise.
  The measured parent/candidate process shrinks `WorkTask.fn` by
  **`1,692 bytes / 117 blocks`**, `Richards.run` by
  **`1,272 bytes / 29 blocks`**, `Packet.append_to` by
  **`936 bytes / 64 blocks`**, `Task.runTask` by
  **`648 bytes / 40 blocks`**, `IdleTask.fn` by
  **`624 bytes / 40 blocks`**, `Task.qpkt` by
  **`556 bytes / 45 blocks`**, and `schedule` by
  **`460 bytes / 31 blocks`**. The disappeared ordinary control-flow and
  memory-access guards are genuine previously retained field
  specializations, not unnecessary code eliminated for free.
- Decoded actual Profile records prove both revisions retain exactly
  **11 concrete owners / 51 type keys** and all **662 `__main__` counter
  rows**, including `WorkTask.fn` **13 sites / 121,718 observations**,
  `Packet.append_to` **4 / 81,416**, `Task.qpkt` **8 / 371,936**, and
  `Task.runTask` **11 / 627,240**. Candidate replay redistributes owners
  from `soac.runtime` **`11 / 51`** to `soac.runtime` **`4 / 32`** plus
  `__main__` **`7 / 19`**; the production `ProfileEvidenceStore` merges
  both correctly. The actual difference is corrected CPython sorting:
  `IdleTaskRec.count` / `WorkerTaskRec.count` change from **`1 / 1`** to
  **`1 / 0`**; task/packet `.ident` changes from five owners all **`1`**
  to four task owners **`1`** plus `Packet` **`2`**; task/packet `.link`
  changes from five owners all **`0`** to four task owners **`0`** plus
  `Packet` **`4`**. The preexisting
  `late_bound_split_owner_nonself_field_plans` condition in
  `crates/soac_opt/src/pipeline_v3.rs` rejects an entire polymorphic load
  whenever owner indices differ, even though existing mechanical JIT
  emission already supports a separately guarded per-owner index.
- Typed IR remains **`2,265 blocks / 183 functions`**. Across all three
  rounds, **10,650 actual JIT rows / 120 Apply PIDs** retain every source
  identity and **`777,240` hidden trampoline bytes**. Native bytes decrease
  **`56,982,240 -> 56,797,080`** and machine blocks
  **`3,727,500 -> 3,716,400`** because profitable mixed-owner guards
  disappear. Median-per-round native is
  **`18,994,080 -> 18,932,360` bytes**, blocks
  **`1,242,500 -> 1,238,800`**, and pre-optimization BlockPy
  **`8,186,192 -> 8,285,072` bytes**. No standard-library or third-party
  hot module is transformed; the other **89 / 97** acceptance variants are
  still unmeasured.
- Result: **retain the CPython-visible correctness repair, disclose the real
  `richards` regression, and immediately restore safe mixed-index
  polymorphic owner guards through the existing hot-nonself strategy**.
  The full `just test-all` correctness gate is still PENDING; no full-suite
  acceptance or performance-neutrality claim is made.

### Independent out-of-scope finding: preexisting class lambda-default lookup

- A separate minimal class-body control contains no `self` stores, inferred
  `__static_attributes__` reference, or static-tail-specific behavior:

  ```python
  shared = "outer"

  class C:
      read = staticmethod(lambda value=shared: value)
  ```

- Stock CPython evaluates `C.read()` to `"outer"`; SOAC instead raises
  `NameError: name 'shared' is not defined`. This reproduces an independent,
  preexisting general class/lambda-default name-resolution bug rather than a
  regression introduced by this class-static-attribute strategy.
- Keep the existing genuine lambda **body** closure regression and its
  repaired static-tail semantics separate from lambda **default-expression**
  lookup. The latter needs a dedicated future repro and lowering fix outside
  this strategy; do not weaken current assertions, expand the candidate to
  absorb unrelated work, or attribute the preexisting failure to this change.

## Verdict and next action

- Verdict: the strengthened focused class matrix passes **`20 / 20`**, all
  four existing owner-field suites pass, and the combined focused gate passes
  **`24 / 24`**. Independent structured lowering and hardened
  callback-free/exact-owner watcher regressions each pass **`1 / 1`**, and
  serial full-crate suites pass **lowering `372 / 372`**, **JIT
  `580 / 580`**, **optimizer `214 / 214`**, and **typed IR `54 / 54`**,
  and the complete broader transformed selection passes
  **`55 / 55` in `99.38 s`** after its genuine historical **`54 / 55`**
  fixture failure is corrected without weakening the negative control.
  The original five-file candidate remains historically **REJECTED
  as-is**. The expanded six-file compatibility candidate is necessary to
  restore user-visible CPython behavior, but its clean same-kernel fixed-four
  stock score is only **`0.5401772590486644x`** versus parent
  **`0.5564929785348224x`**, official previous SOAC
  **`0.9744429326747311x`**. Correct CPython key sorting causes the existing
  uniform-only non-self planner to drop real `richards` guards, with
  reproducible stock-paired **`0.940916x`** performance
  (**95% `0.918484-0.965860`**) in every round. Preserve compatibility and
  recover the reusable mixed-index specialization; do not claim this change
  is performance-neutral. Combined lowering/JIT test-target and
  package-scoped format checks both pass. The final authoritative combined
  `just test-all` gate is **GREEN**: **1,259 transformed nodeids / 101
  isolated batches / 8 workers / 101 PASS / 0 failures**, plus **54 typed
  IR**, **580 JIT**, **372 lowering**, **214 optimizer**, and **8 PyO3**
  tests; evidence is `work/logs/class-static-mixed-owner-test-all.log`.
  Debug-runtime setup takes **24.716 seconds**, Cargo tests
  **81.230 seconds**, inner / outer pytest **78.940 / 78.954 seconds**,
  and total test phase **160.193 seconds**. The existing 28-node
  counter-dump shard takes **78.22 seconds**. Full-suite `1.10x`
  acceptance remains unmet and unmeasured; no landing claim is made.
- Transferable lesson: compiler-generated `__static_attributes__` is the final
  unconditional lexical class-body result; an explicit empty class-body value
  cannot suppress CPython's own tuple or its shared-key preseeding.
- Next action: preserve the completed **`24 / 24`** focused gate, all four
  restored owner-field suites with exact-owner filtering and truthful
  CPython indices, genuine lambda/method closure RED-to-GREEN controls,
  independently GREEN hardened callback-free/uninstantiated-owner structured
  watcher, historical **`18 / 18`** / `16 / 16` checkpoints, and
  **`1 / 1`** structured lowering GREEN, corrected semantic-role lowering
  regression, and serial **`372 / 372`**, **`580 / 580`**, **`214 / 214`**,
  and **`54 / 54`** crate suites, and the strict unequal-index `_padding`
  control within the **`55 / 55`**, **`99.38 s`** broader regression gate.
  Preserve the passing combined lowering/JIT test-target and scoped-format
  checks and actual **8 / 8** release/normal plus **4 / 4** same-kernel
  three-round coverage. The subsequently reopened existing hot-nonself
  strategy now independently restores safe bounded mixed-index owner guards
  without undoing the required CPython class layout. Its genuine whole-plan
  and real transformed regressions turn GREEN, and clean repeated
  `comparison-20260820-102351-dmaMsN` improves the class-correct
  intermediate stock score **`0.5401772590486644x ->
  0.5596865226885351x`**, official intermediate previous SOAC
  **`1.0572879104903203x`**. Versus the original integrated parent, stock
  changes **`0.5564929785348224x -> 0.5596865226885351x`**, but
  stock-paired `richards` still regresses **`0.9502x`**
  (**95% `0.9241-0.9896`**) despite restored mixed guards; do not claim
  complete recovery. Combined optimizer/JIT test-target checking passes
  **`9.22 seconds`**, and the final combined `just test-all` gate passes
  **1,259 nodeids / 101 batches / 8 workers / 0 failures**. Investigate the
  remaining parent-relative paired `richards` cost and run all 97
  full-pyperformance variants before any acceptance claim.
