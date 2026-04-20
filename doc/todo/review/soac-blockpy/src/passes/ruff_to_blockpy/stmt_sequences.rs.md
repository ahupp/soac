# soac-blockpy/src/passes/ruff_to_blockpy/stmt_sequences.rs

## File Responsibilities

Lowers statement lists into labeled BlockPy regions. It scans linear prefixes until control-flow
heads, emits blocks for terminators, branches, loops, try/with expansion, and connects regions
through `RegionTargets`.

## Datatypes

- `InstrSequenceDriveResult`: result of scanning a statement sequence, either exhausted with a
  linear prefix or stopped at a control-flow head with its index and plan.

## Functions

- `lower_stmts_to_blockpy_stmts_with_context`: lowers a list of `InstrRuff` statements into one
  inline builder without external control-flow region handling.
- `plan_instr_sequence_head`: delegates one-statement planning to statement lowering.
- `drive_instr_sequence_until_control`: consumes linear statements until it finds a non-linear
  statement head or expansion.
- `compat_blockpy_raise_from_instr`: converts a no-cause raise statement to a raise terminator.
- `contains_return_instr_in_body`, `contains_return_stmt_in_handlers`,
  `contains_return_stmt_in_body`, `contains_return_instr`: detect returns inside structured
  statements for finally/control-flow planning.
- `lower_common_stmt_sequence_head`: handles raise, return, if, while, break, and continue heads
  shared by multiple sequence paths.
- `lower_for_stmt_sequence_head`: allocates labels/temps and delegates full for-loop lowering.
- `lower_stmt_sequence_with_state`: public entrypoint for lowering a statement list with region
  targets.
- `lower_instr_stmt_sequence_with_state`: recursive sequence driver that handles linear prefixes,
  with/for/try/expanded/control heads, and final jumps.
- `lower_expanded_stmt_sequence`: prepends expanded statements to remaining statements and emits a
  linear-prefix jump into the expanded entry.
- `lower_if_stmt_sequence`: lowers then/else regions and emits the if-test branch block.
- `lower_if_stmt_sequence_from_stmt`: prefers inline-fragment lowering for if statements, falling
  back to region lowering.
- `lower_while_stmt_sequence`: lowers loop body, else body, rest continuation, and while test.
- `lower_while_stmt_sequence_from_stmt`: extracts while fields and delegates loop lowering.
- `lower_for_stmt_exit_entries`: lowers rest and optional for-else exits.
- `lower_for_stmt_body_entry`: lowers a for-loop body with break/continue loop targets.
- `lower_for_stmt_sequence`: combines for-loop exits, body, setup, iteration check, and target
  assignment blocks.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/compat.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/try_regions.rs`
- `soac-blockpy/src/block_py/mod.rs`

