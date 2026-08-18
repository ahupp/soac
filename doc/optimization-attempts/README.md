---
title: "Optimization Strategy History"
---

# Optimization strategy history

This directory contains durable, tracked investigation records for attempted
SOAC optimization strategies. Each strategy has exactly one Markdown file,
updated across all of its implementation attempts and benchmark runs.

Name strategy files `YYYY-MM-DD-<strategy-slug>.md`, using the date in the
Pacific timezone. Start from `TEMPLATE.md`. If the same strategy is revisited,
update its existing file rather than creating another file for a new run.

Create a strategy record before changing code or immediately after selecting
the strategy. Keep it even if all experimental changes are reverted. Record
positive, negative, failed, rejected, and inconclusive attempts; the reason an
approach did not work is often as valuable as the retained implementation.

Each file must include:

- A specific hypothesis, expected general-purpose benefit, and supporting
  profile, generated-code, or structural evidence.
- The implementation shape and relevant CPython-compatibility analysis,
  including mutable assumptions, guard lifetime, visible effects, fallback,
  and focused regression coverage.
- The fixed benchmark set and protocol, comparison round count, stock CPython
  result, previous SOAC baseline, candidate results, relative deltas, and
  whether the result is statistically meaningful. Label missing or pending
  values instead of guessing.
- Completed and failed benchmarks plus benchmark-specific transformed project,
  dependency, and standard-library coverage; distinguish benchmark completion
  from meaningful JIT coverage.
- Available pre-optimization serialized BlockPy bytes, optimized typed-IR
  instruction or basic-block counts, apply-mode native code bytes and machine
  blocks, and material startup/compilation costs.
- A chronological attempt history, current status, final verdict, transferable
  lessons, and concrete next action.

Use Pacific time with an explicit timezone whenever recording a timestamp, for
example `2026-08-18 08:30 PDT`. Large benchmark outputs and profiles stay
under ignored `work/`; include their paths for provenance, but copy essential
numbers and conclusions into the tracked strategy file.

`doc/PERF_LOG.md` is a separate concise summary of finalized retained
performance changes. It is not the detailed investigation log and does not
replace records of strategies that were rejected or reverted.
