# scripts/run_pytest_parallel.py

## File Responsibilities

Runs pytest across batches in parallel while preserving deterministic output and useful failure summaries. It collects node IDs when simple selectors allow splitting and falls back to a single pytest run for complex selectors.

## Datatypes

- `RunResult`: subprocess result for one pytest batch, including selector, return code, and captured output.
- `PytestBatch`: named group of pytest node IDs or file selectors.
- Module constants: repository/venv paths from environment and minimum node-id batch size.

## Functions

- `parse_jobs`: clamps requested worker count to available CPU/job bounds.
- `is_simple_selector`: identifies selectors that can safely participate in node-id collection.
- `collect_test_nodeids`: runs pytest collection and returns node ids plus collection output.
- `node_group_key`: groups parametrized/class test node ids by file/class/function prefix.
- `node_file_path`: extracts the source file component from a node id.
- `make_file_batches`: packs files and node groups into balanced batches; nested `flush_current_batch` emits the current batch.
- `make_nodeid_batches`: round-robins node ids into worker batches.
- `pytest_cmd`: builds a repo-local venv pytest command.
- `run_pytest`: executes one pytest selector batch with captured output.
- `print_failure`: prints captured stdout/stderr for a failing batch.
- `main`: chooses serial/parallel strategy, runs batches, reports failures, and returns an aggregate status.

## Context Read

- `Justfile` pytest recipes
- `AGENTS.md` testing entrypoint notes

