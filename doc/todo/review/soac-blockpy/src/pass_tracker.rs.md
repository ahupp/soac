# soac-blockpy/src/pass_tracker.rs

## File Responsibilities

Pass tracking infrastructure for lowering. It records named pass outputs and timings for debugging,
snapshots, tools, and `diet-python`, while allowing production callers to use a no-op tracker.

## Datatypes

- `PassTiming`: pass name and elapsed duration.
- `TrackedPass`: erased cloned pass output plus render functions.
- `NoopPassTracker`: tracker that runs passes without storing data.
- `RecordingPassTracker`: tracker that stores pass outputs and timings.
- `PassTracker`: internal trait abstracting pass execution and timing.

## Functions

- `BlockPyPrettyPrint for Suite` / `ModModule`: render Ruff AST pass outputs as Python source.
- `render_tracked_pass_value`, `render_tracked_pass_debug_value`: downcast erased pass outputs and
  render them.
- `NoopPassTracker::new`: constructs no-op tracker.
- `NoopPassTracker::run_pass`, `record_timing`: simply execute closures.
- `RecordingPassTracker::new`: constructs empty recording tracker.
- `RecordingPassTracker::record_pass_timing`: appends unique pass timing.
- `RecordingPassTracker::get`: typed lookup by pass name.
- `RecordingPassTracker::pass_names`: iterate recorded pass names.
- `RecordingPassTracker::pass_ast_to_ast`, `pass_core_blockpy`,
  `pass_core_blockpy_with_await_and_yield`, `pass_name_binding`: typed convenience accessors for
  common passes.
- `RecordingPassTracker::render_pass_text`, `render_pass_debug_text`: render recorded passes.
- `RecordingPassTracker::pass_timings`: iterate timing records.
- `RecordingPassTracker::run_pass`: time, clone, erase, and store a pass output.
- `RecordingPassTracker::record_timing`: measure closure elapsed time and record it.

## Context Read

- `soac-blockpy/src/block_py/pretty/mod.rs`
- `soac-blockpy/src/driver.rs`
- `soac-blockpy/src/lib.rs`
