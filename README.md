# Plan


Python is a surprisingly complicated language, and to run it fast it first needs
to be made into a smaller language.  There are a few phases here:

python -> python:
  * Strip annotated assignments ("x : int = 1"), and emit as `__annotate__` / `__annotate_func__`.
  * Rewrite "private" names in classes like `__foo` -> `_{classname}_foo`.
  * `assert` -> `if __debug__`
  * `if..elif` into a chain of `if..else`
  * type aliases and parameters into calls to `TypeVar` / `TypeParam` etc.
  * multi-target assign and delete to single target + temporaries
  * f-strings to explicit string formatting
  * augassign and operators -> explicit function calls

python -> bb python
  * flow control: for/while/with



# diet-python

This repository includes a small Rust utility for transforming Python source
code. It parses a file with Ruff's parser and rewrites binary operations and
augmented assignments (e.g., `+=`) into calls to the corresponding functions in
the standard library's `operator` module. The transformation is idempotent, so
re-running it on already rewritten code leaves the output unchanged.


Run it with:

```
cargo run --bin diet-python -- path/to/file.py
```

## Python import hook

To apply the transform automatically when modules are imported, install the
provided import hook:

```python
from soac import import_hook
import_hook.install()
```

After calling `install()`, any subsequent imports will be rewritten using the
`diet-python` transform before execution.

Run the included example to see the hook in action:

```
python example_usage.py
```

The script installs the hook, imports `example_module`, and asserts that its
bytecode calls `operator.add` instead of using `BINARY_OP`.

## Regenerating transform fixtures

If a transform change updates the expected desugaring, regenerate the fixture
outputs with:

```
cargo run --bin regen_snapshots
```

# CLIF

```
$ rustup component add rustc-codegen-cranelift-preview --toolchain nightly
$ ./rust-clif-dist/rustc-clif --out-dir=clif-out/ --crate-type=rlib fastadd.rs -Cdebuginfo=0 --emit link,llvm-ir
```

# Log

2026-01-15:
  - Totals: duration 18m 3s; tests run 37,414; failures 747; skipped 1,706; test files run 483/492; failed 103; env_changed 1;
    skipped 31; resource_denied 9
2026-01-16:
  - Test files: 401 passed / 492 total (483 run; 81 failed; 1 env_changed; 31 skipped; 9 resource_denied).
  - Test cases: 39,237 passed / 39,820 total (583 failed; 1,835 skipped).

Then
• Test File Counts

  - Passing: 388/492
  - Run: 483/492
  - Failed: 95
  - Skipped files: 44

  Individual Test Cases

  - Run: 39,320
  - Passed: 38,685
  - Failed: 635
  - Skipped: 1,754

2026-01-17:
Total duration: 33 min 49 sec
Total tests: run=28,491 failures=612 skipped=1,426
Total test files: run=488/492 failed=160 skipped=24 resource_denied=4
Result: FAILURE


2026-02-02:

Total duration: 48 min 11 sec
Total tests: run=32,863 failures=705 skipped=1,778
Total test files: run=483/491 failed=132 skipped=27 resource_denied=8
Result: FAILURE


# Perf


2026-02-04: First run of transformed interpreter path

transformed interpreter
15880011868 3148 loops/s
stock cpython
967408991 1033688 loops/s
transform-only
1001108600 998892 loops/s

= 328x slower

2026-03-05: Full JIT

- Warmed in-process comparison:
  - JIT: logs/benchmark_jit_warm.log -> 5238 loops/s
  - Stock: logs/benchmark_stock_warm.log -> 824962 loops/s
= 157x slower

Vectorcall entry:

  Relative performance:

  - Stock is 95.71x faster than current JIT path
  - JIT is 1.045% of stock throughput

Use C API for operators:

  Relative:

  - JIT is 1.940% of stock throughput
  - Stock is 51.54x faster on this pystone run

2026-03-08: Remove tuple state passing between blocks:

  - JIT transformed: 23684 loops/s
  - Stock CPython: 913322 loops/s

  Relative:

  - stock is 38.56x faster
  - JIT is 2.59% of stock throughput

2026-03-25:  whole lot of cleanup, no perf work

• Current cold comparison from logs/benchmark-pystone-compare-20260325.log:

  - jit transformed: 30,536 loops/s
  - stock cpython: 906,698 loops/s

  Relative:

  - Stock is 29.69x faster
  - JIT transformed is 3.37% of stock throughput


2026-04-03:

changes:
  - refcounting as cranelift functions, constant pool for all strings
    - JIT/transformed: 105,083 loops/s
    - Stock CPython: 830,761 loops/s
    - transformed is about 0.126x stock, so stock is 7.9x faster.

  - 40e43654 Use Cranelift speed opt level and native ISA for JIT benchmarks
      - transformed/JIT: 91,257 loops/s
      - stock CPython: 754,886 loops/s
      - transformed is 0.121x stock, so stock is about 8.27x faster
      - timing: real 10.75, user 14.99, sys 0.83
      - log: logs/benchmark_opt_native_20260403.log
  - 404cbee4 Inline runtime CLIF support helpers into JIT callers
      - transformed/JIT: 119,398 loops/s
      - stock CPython: 739,834 loops/s
      - transformed is 0.161x stock, so stock is about 6.20x faster
      - timing: real 9.54, user 14.81, sys 0.76
      - log: logs/benchmark_opt_native_inlining_20260403.log
  - lift runtime functions to constants, immortal constants
      - transformed/JIT: 175,380 loops/s
      - stock CPython: 759,045 loops/s
      - transformed is about 0.231x stock
      - stock is about 4.33x faster
  - write through globals cache
      - transformed/JIT: 177,856 loops/s
      - stock CPython: 745,030 loops/s
      Relative performance:  transformed is about 0.239x stock stock is about 4.
  - really use vectorcall
      - transformed/JIT: 221,433 loops/s
      - stock CPython: 892,476 loops/s
      - transformed is about 0.248x stock, so stock is about 4.03x faster
  - fix read through globals cache, near 100% hitrate
      - JIT transformed: 245,183 loops/s
      - Stock CPython: 954,347 loops/s
      - So without counters enabled, the JIT is about 25.7% of stock throughput, or 3.89x slower.

# Design

Dropping to basic block format:

 - gave control over name binding for functions
 - significantly improved fidelity to flow control, made generators easier, and reduced JIT surface area


 # Principles

  * Locality: for any specific concept, it's better to handle it in one place.
    e.g, prefer to handle different kinds of load/store (global, nonlocal,
    local, class-body) in one place, rather than spreading them across many
    different transforms.  For example, things we prefer not to do:
      - have many different layers of the system aware of annotations and annotationlib
      - special cases that match on specific internal variable names
      - many different sites aware of scoping rules
  *


# Optimizations

 * Inlining
   * Is there only one caller?
   * Is it below < size?
   * Does it unlock other optimizations?
 * Specialization
    * Known cell address
    * Call fastpath knowing exact sig of target
    * Unboxing

 * Can we skip deleted checks on this value?

 * Minimize refcounting
 * Code size/locality
 * Maximize register use
 * Avoid constant exception checking
 * Flow control exceptions to jumps
 * Compile-time computation
 * Known subclasses
   * No overrides to function
 * Type hints enforcement
 * Escape analysis, stack allocate
   * Inline closure cells
 * Green threads for async

## Facts
 * Constant
 * ReadOnly
 * ExactTypes(...)
