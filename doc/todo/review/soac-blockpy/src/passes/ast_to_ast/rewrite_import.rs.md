# soac-blockpy/src/passes/ast_to_ast/rewrite_import.rs

## File Responsibilities
Rewrites Python import statements into explicit calls to SOAC runtime import helpers, making import behavior visible to later lowering passes.

## Datatypes
- None.

## Functions
- `rewrite`: lowers `import ...` statements to assignments from `__soac__.import_`, including dotted import alias handling through repeated `import_attr` calls.
- `rewrite_from`: lowers `from ... import ...` statements, including import-star handling and relative import levels, using a temporary module binding followed by attribute loads.

## Context Read
- `soac_py/src/soac/runtime.py` import helper functions.
- `ast_rewrite/mod.rs` for `Rewrite::Walk` statement replacement.
