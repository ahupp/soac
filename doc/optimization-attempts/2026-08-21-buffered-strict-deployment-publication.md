---
title: "Buffer strict deployment publication"
---

# Buffer strict deployment publication

- Status: in progress
- Pacific date: 2026-08-21 PDT
- Change or revision: shared strict-runtime implementation; not yet finalized
- Outcome: serializer, 23 CLI tests, and genuine 26-module publication pass; elapsed speedup unmeasured

## Hypothesis and evidence

The offline CLI serialized its startup descriptor directly into a
`NamedTempFile`. Serde's small writes therefore crossed the mounted-checkout
filesystem separately. A genuine 26-module publication exceeded the test
helper's 180-second setup limit after publishing every signed shard and the
generation manifest; the private descriptor file contained 7.29 MB of truncated
JSON when the process was killed. No usable startup authority was published.

Buffering this sequential serialization should reduce file-write calls without
changing any artifact bytes, observations, or atomic publication rules. This is
an offline workflow improvement, not a steady-state JIT optimization or evidence
of progress toward the pyperformance geometric-mean target.

## Implementation and compatibility

- Buffer serialization and the final newline; explicitly propagate flush errors
  before syncing the file, revalidating inputs, and atomically persisting it.
- Preserve private temporary-file cleanup, complete-generation publication,
  error propagation, and the absence of fabricated startup authority.
- Keep all source/dependency observations and their revalidation unchanged.
  No runtime capability, Python callback, ownership, or execution path changes.
- Focused coverage compares complete emitted bytes with the original serializer,
  counts underlying writes, and injects a flush failure before sync/persist.
  Both new tests and all **23 CLI tests pass**. The error case covers both an
  absent destination and preservation of an existing descriptor, plus removal
  of the private staging file.

## Benchmark protocol and coverage

- Fixed setup workload: the same reviewed 26 source bodies as
  `tests/test_strict_entry_runtime.py`, genuinely analyzed by the pinned checker.
- Before evidence: the mounted-checkout publication timeout in
  `work/logs/strict-loader-snapshot-v8-republication.log`; this is not a completed
  timing sample or a controlled speedup baseline.
- Candidate publication: succeeds for all 26 modules at
  `/tmp/soac-strict-loader-snapshot-v8-01a02587`. The full fixture is archived at
  `work/strict-loader-snapshot/v8-comparison-archive`, retaining its original
  absolute path identities. Moving from mounted to guest-local storage also
  changed the setup environment, so this is not a controlled elapsed comparison.
- Stock CPython, SOAC profile/apply, previous/candidate pyperformance rounds:
  unavailable for this offline serialization operation. No Python module body
  is imported or transformed during publication.
- Typed IR, BlockPy size, native code bytes, and machine blocks: not applicable;
  this change emits no runtime code. No throughput or suite-wide claim is made.

## Measurements

| Metric | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| Mounted-checkout 26-module CLI setup | exceeded 180 s, incomplete descriptor | pending | no speedup claim |
| Underlying writes for fixed serializer fixture | 40,002 | 34 | structural, not elapsed time |
| Fixed serializer payload | 270,002 bytes | byte-identical 270,002 bytes | identical |
| Explicit flush error before publication | new contract fails without explicit flush | propagated; old destination preserved | fail closed |
| Stock / SOAC / previous-candidate pyperformance | unavailable | unavailable | not claimed |
| Generated IR and native code size | not applicable | not applicable | no runtime code change |

## Attempt history

### Attempt 1: explicit buffered flush before publication

The initial timeout was first suspected to involve project discovery because
the fixture moved from pytest's guest-local temporary directory into `work/`.
Completed signed shards and a truncated temporary descriptor localized the
failure to publication instead. The temporary descriptor is diagnostic evidence
only; it is never supplied to the runtime as authority.

Both focused tests failed against the original unbuffered helper
(`work/logs/strict-deployment-buffered-before.log`): the fixed payload generated
40,002 underlying writes, and the newly required explicit flush was absent.
The latter is protection against introducing delayed-error loss with buffering,
not a claim that the old unbuffered `File` had a userspace flush bug. After the
change, all 23 CLI tests pass in 37.01 seconds
(`work/logs/strict-deployment-buffered-after.log`), including genuine selected
CPython/dependency/policy invalidation and atomic-publication checks. The fixed
payload uses 34 underlying writes with exactly unchanged bytes. A real candidate
26-module publication then succeeded
(`work/logs/strict-loader-snapshot-v8-guest-local-publication.log`): the final
descriptor is 7,580,839 bytes, and its archived copy is byte-identical. It records
67 file, four directory, and 20 missing inputs, with only the explicit project
configuration; it did not recursively analyze the checkout. Unit-test elapsed
time and this guest-local publication are not a setup speedup comparison.

## Verdict and next action

Focused validation is complete; shared integration/full-gate validation remains
pending. A serializer writing
to a mounted filesystem needs explicit buffering and error-checked flushing;
increasing a timeout alone would leave the underlying workflow defect intact.
