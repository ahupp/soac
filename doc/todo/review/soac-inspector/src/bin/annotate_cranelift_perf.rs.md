# crates/soac_inspector/src/bin/annotate_cranelift_perf.rs

## File Responsibilities

Annotates rendered Cranelift VCode with perf sample counts for SOAC JIT code. It correlates `perf script` output against `jit_bb_map.jsonl`, aggregates samples by function and block, writes annotated VCode files, and emits a JSONL performance annotation summary.

## Datatypes

- `JitBbMapRecord`: one JIT function address/size/block-map record from benchmark artifacts.
- `JitCodeKey`: stable key for a JIT DSO symbol path plus symbol name.
- `PerfSample`: parsed perf sample address, symbol offset, and sample period.
- `BlockKey`: key for a specific block within a JIT function.
- `BlockCount`: aggregate samples/self weight for a block.
- `FunctionBlockKey`: key for function-local block-count summaries.
- `FunctionBlockCount`: block sample aggregate enriched with function metadata.
- `FunctionInfo`: function id/name/module metadata loaded from rendered CLIF function listings.

## Functions

- `main`: drives result-directory discovery, artifact parsing, perf correlation, annotated-file output, and summary generation.
- `parse_result_dir`: reads the benchmark result directory argument.
- `load_jit_bb_maps`: parses `jit_bb_map.jsonl` into records keyed by JIT DSO/symbol.
- `perf_jit_samples`: runs `perf script` and parses JIT samples.
- `parse_perf_sample_line`: extracts one JIT sample from a perf-script line.
- `parse_jitted_dso_key`: extracts `JIT-<pid>-<index>.so` keys from perf DSO paths.
- `block_for_offset`: maps a symbol offset to the covered Cranelift block.
- `write_annotated_vcode_files`: writes one annotated VCode file per sampled function.
- `find_vcode_path`: locates the rendered VCode file for a function-local id.
- `load_functions`: loads function metadata from inspector function-list JSONL.
- `annotate_vcode`: inserts sample-count comments beside VCode block labels.
- `write_perf_annotation`: writes function/block sample summaries as JSONL.
- `parse_vcode_block_label`: extracts block labels from VCode text.
- `function_local_id`: strips module prefixes from packed function ids for file matching.
- `percent`: computes a percentage while handling zero totals.

## Context Read

- `scripts/summarize_benchmark_result.py`
- `crates/soac_inspector/src/bin/render_jit_clif.rs`
- `soac_jit` JIT BB map artifact shape

