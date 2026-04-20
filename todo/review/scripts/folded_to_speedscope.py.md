# scripts/folded_to_speedscope.py

## File Responsibilities

Converts folded stack samples into a speedscope-compatible sampled profile JSON document. It normalizes sample weights so large count ranges produce reasonably sized profile files.

## Datatypes

- Module constant `TARGET_TOTAL_WEIGHT`: default total sample count after normalization.
- No classes are defined.

## Functions

- `parse_folded_stacks`: parses `frame;frame count` lines into frame table, sample frame indexes, and raw weights.
- `normalize_weights`: rescales raw weights to a target total while preserving at least one sample per nonzero stack.
- `build_sampled_profile_output`: constructs the speedscope JSON schema with frames, samples, weights, and profile metadata.
- `main`: reads folded stacks from stdin and writes speedscope JSON to stdout.

## Context Read

- Speedscope folded-stack/profile format usage in benchmark/perf scripts.

