# scripts/pystone.py

## File Responsibilities

Repo-local copy of the classic Pystone benchmark. It is used as SOAC's primary performance workload and intentionally keeps old benchmark structure, globals, and procedure names so optimizations can target stable hot paths.

## Datatypes

- `Record`: benchmark record object with pointer, discriminator, enum, integer, and string fields; its `copy` method constructs another `Record`.
- Module constants/globals: `LOOPS`, enum-like integer constants, global scalars, arrays, and record pointers used by benchmark procedures.

## Functions

- `Record.__init__`: initializes benchmark record fields.
- `Record.copy`: returns a shallow copy preserving all record fields.
- `main`: prints benchmark timing for a loop count.
- `pystones`: executes `Proc0` and returns elapsed time plus loops/second.
- `Proc0`: top-level benchmark loop that constructs globals, repeatedly calls procedures/functions, and computes throughput.
- `Proc1`: mutates linked `Record` objects and dispatches through `Proc3`, `Proc6`, and `Proc7`.
- `Proc2`: integer loop with global character check and arithmetic.
- `Proc3`: updates pointer output and `PtrGlb.IntComp`.
- `Proc4`: updates global boolean and character values.
- `Proc5`: resets global character and boolean values.
- `Proc6`: enum-dispatch procedure using `Func3` and globals.
- `Proc7`: small integer arithmetic helper.
- `Proc8`: array mutation helper for one- and two-dimensional globals.
- `Func1`: character comparison helper returning enum-like results.
- `Func2`: string comparison loop helper.
- `Func3`: enum equality helper.
- nested `error`: local helper in the `__main__` verification block that reports unexpected benchmark values.

## Context Read

- `just benchmark`
- SOAC optimization docs that use Pystone as the default benchmark.

