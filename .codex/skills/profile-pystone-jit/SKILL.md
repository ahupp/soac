---
name: profile-pystone-jit
description: Profile this repo's warmed JIT pystone path with Linux perf. Use when Codex needs to run pystone under perf record, dump perf report artifacts into logs/, inspect DSO and symbol hotspots, explain where time is spent between python, libdiet_python.so, and [JIT], or suggest performance work based on the captured profile.
---

# Profile Pystone Jit

Use `scripts/perf_pystone_jit_warm.sh` from the repo root. The script builds the release extension, warms pystone in-process, records the measured run under `perf`, and writes report artifacts into `logs/`.

## Run

Use the warmed default unless the user requests something else:

```bash
just perf-pystone-jit-warm 10000000 logs/pystone_jit_perf_warm_run
```

For a quick smoke test after changing the profiler script:

```bash
./scripts/perf_pystone_jit_warm.sh 1000 logs/pystone_jit_perf_warm_smoke
```

## Outputs

Expect these files for a given prefix:

- `<prefix>.log`
- `<prefix>_report.txt`
- `<prefix>_by_dso.txt`
- `<prefix>_by_dso_symbol.txt`
- `<prefix>_callgraph.txt`

Keep all profiling artifacts in `logs/`.

## Summarize

When reporting results:

- Read loops per second from `<prefix>.log`.
- Read overall DSO split from `<prefix>_by_dso.txt`.
- Read top self symbols from `<prefix>_by_dso_symbol.txt`.
- Read cumulative hot paths from `<prefix>_callgraph.txt`.
- State clearly whether the time is mostly in `python`, `libdiet_python.so`, or `[JIT]`.

Common interpretations:

- High `load_name_hook`, `PyObject_CallFunctionObjArgs`, `object_vacall`, `PyDict_GetItem*`, `unicode_decode_utf8`, or refcount and GC symbols means boundary overhead dominates.
- High `[JIT] dp_jit_run_bb_specialized` with relatively low CPython/runtime overhead means generated code is the main cost.

## Notes

- Expect kernel symbol warnings when `kptr_restrict` hides kernel maps. User-space symbol resolution is still enough for this workflow.
- If the user asks for optimization suggestions, prioritize the largest diet-python-specific hotspots before generic CPython costs.
