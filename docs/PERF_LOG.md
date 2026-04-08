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

2026-04-06: better specialization coverage, and constant string interning:
  - transformed/JIT profile pass: 221,542 loops/s
  - transformed/JIT specialized pass: 294,980 loops/s
  - stock CPython: 864,134 loops/s

  Headline comparison:

  - specialized transformed is about 0.341x stock
  - stock CPython is about 2.93x faster

2026-04-07: new baseline on VM
  profile pass:      118,016 loops/s
  specialized pass:  201,970 loops/s
  stock CPython:     620,991 loops/s
  hot callsites:     22

  Specialized baseline is ~32.5% of stock.

# Unsound

`SOAC_OPT_UNSOUND=1` enables broad compatibility-skipping performance
experiments. It is enabled by default in benchmark and pytest
correctness recipes so ordinary transformed test runs still detect
unexpected fallout from these experiments.

When enabled, specialized JIT code may call `soac-runtime` helpers that
overwrite an existing guarded storage slot directly:

- indexed module-global store: replace `PyDictIndexedValues.values[index]`
- split instance-field store: replace `PyDictValues.values[index]`

The helper still returns null and uses the ordinary slow path for
first-insert stores, missing value slots, global tombstones,
promoted/combined instance dictionaries, guard misses, type-version
misses, and split-key index mismatches.

This is not CPython-compatible. The raw store skips dict/object/type
watchers, dict version updates, insertion-order maintenance,
`ma_used`/first-insert bookkeeping, and any future CPython bookkeeping
that lives in the normal store path. Use it only in perf runs that
explicitly want to estimate the upper bound for guarded indexed stores;
do not enable it for correctness tests or compatibility claims.

Current behavior:

- guarded existing module-global store may replace
  `PyDictIndexedValues.values[index]` directly
- guarded existing split instance-field store may replace
  `PyDictValues.values[index]` directly
- undeclared known-builtin global loads are rewritten during name
  binding into `RuntimeName` loads; module-constant extraction then
  snapshots the runtime/builtin object in a constant slot

This is not CPython-compatible. A module can install or mutate a global
named `len`, `range`, `print`, etc. through `globals()`, `exec`, custom
module initialization, or ordinary code that this static declaration
check does not see. That mutation should shadow builtins for normal
Python global lookup, but the unsound runtime-name path does not check
module globals.
