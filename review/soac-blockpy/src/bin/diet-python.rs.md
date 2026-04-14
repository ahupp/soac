# soac-blockpy/src/bin/diet-python.rs

## File Responsibilities

Command-line tool that reads one Python file, runs the normal BlockPy lowering pipeline used for
testing, and prints the post AST-to-AST Python source. With `--timing`, it emits JSON timing data
for the full lowering and each tracked pass to stderr.

## Datatypes

- `USAGE`: command usage string.

## Functions

- `main`: parses `--timing`, `--help`, and the single input path; reads source; lowers it with
  `lower_python_to_blockpy_for_testing`; prints the recorded `ast-to-ast` pass as Python source;
  and optionally writes timing JSON containing `total_ns` and per-pass `elapsed_ns`.

## Context Read

- `soac-blockpy/src/lib.rs`
- `soac-blockpy/src/pass_tracker.rs`
- `soac-blockpy/src/driver.rs`
