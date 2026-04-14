# soac-blockpy/src/bin/regen_snapshots/mod.rs

## File Responsibilities

Snapshot regeneration CLI for BlockPy fixture files. It finds `snapshot_*.py` fixtures, parses
fixture blocks, lowers each input through the testing pipeline, writes normalized fixture inputs and
rendered snapshot outputs, formats the touched Python files with `ruff format`, and writes
`snapshot/snapshot_summary.txt` with BlockPy/CLIF block counts or errors.

## Datatypes

- `SnapshotSummaryRow`: one row of snapshot regeneration summary data, including the qualified case
  name, optional BlockPy/CLIF block counts, and optional compact error text.

## Functions

- `collect_fixtures`: recursively discovers snapshot fixture Python files.
- `repo_root`, `snapshot_dir`, `fixture_root`: derive repository and snapshot paths from
  `CARGO_MANIFEST_DIR`.
- `snapshot_output_path_for_fixture`: maps a fixture path to its generated snapshot output path.
- `qualified_case_name`: combines fixture file stem and fixture block name for summaries.
- `render_blockpy_snapshot`: selects the recorded core BlockPy render and counts BlockPy/codegen
  blocks.
- `panic_payload_message`, `format_snapshot_error_message`, `summary_error_text`: normalize panic
  and lowering failures for snapshot output and summary text.
- `with_suppressed_panic_hook`: runs snapshot lowering while catching panics without printing the
  default panic hook output.
- `count_clif_blocks`: counts codegen blocks across all callable definitions.
- `write_if_changed`: atomically avoids rewriting unchanged generated files.
- `render_snapshot_python_fixture`: writes generated snapshot fixtures with expected-output lines
  commented.
- `is_fixture_header_line`, `next_nonempty_line`, `is_snapshot_block_header`: recognize fixture
  block boundaries in generated snapshot files.
- `parse_snapshot_fixture`: parses a generated snapshot file back into fixture blocks, preserving
  only names and inputs.
- `load_fixture_blocks`: chooses parser for source fixtures versus generated snapshot fixtures.
- `regenerate_fixture`: runs the end-to-end regeneration flow for one fixture file and appends
  summary rows.
- `format_python_files`: invokes `ruff format` on fixture and snapshot Python files.
- `write_summary`: writes `snapshot_summary.txt`, disambiguating duplicate case names.
- `main`: initializes logging, chooses fixtures from args or recursive discovery, regenerates all
  fixtures, formats outputs, and writes the summary.

## Context Read

- `soac-blockpy/src/fixture.rs`
- `soac-blockpy/src/lib.rs`
- `soac-blockpy/src/pass_tracker.rs`
- `soac-blockpy/src/block_py/pretty/mod.rs`
