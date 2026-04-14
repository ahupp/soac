# soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/if_stmt/mod.rs

## File Responsibilities

Normalizes `elif` chains and lowers if-statements into inline BlockPy fragments when their bodies
are compatible with structured inline lowering. It supports both raw AST statements and `InstrRuff`
statements.

## Datatypes

- None.

## Functions

- `expand_if_chain`: rewrites `elif` clauses into nested `if` statements in the else branch.
- `try_lower_if_stmt_fragment`: simplifies and lowers a raw AST if-statement into an inline
  fragment when possible.
- `try_lower_if_instr_fragment`: lowers an `InstrRuff` if-statement into an inline fragment.
- `lower_simplified_if_stmt_fragment`: lowers a simplified raw AST if into test/body/else fragments
  and connects them with an if terminator.
- `lower_simplified_if_instr_fragment`: same as above for `InstrRuff` bodies.
- `lower_nested_body_to_inline_fragment`: lowers a raw AST body if every statement is inline
  compatible.
- `lower_nested_instr_body_to_inline_fragment`: lowers an `InstrRuff` body if every statement is
  inline compatible.
- `lower_orelse_to_inline_fragment`: lowers raw AST else clauses or creates an empty fallthrough
  block.
- `lower_instr_orelse_to_inline_fragment`: lowers `InstrRuff` else bodies or creates an empty
  fallthrough block.
- `suite_is_inline_fragment_compatible`, `stmt_is_inline_fragment_compatible`: raw AST compatibility
  checks for inline fragment lowering.
- `instr_suite_is_inline_fragment_compatible`, `instr_stmt_is_inline_fragment_compatible`:
  `InstrRuff` compatibility checks for inline fragment lowering.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_sequences.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/mod.rs`

