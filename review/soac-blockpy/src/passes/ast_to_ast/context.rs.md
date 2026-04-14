# soac-blockpy/src/passes/ast_to_ast/context.rs

## File Responsibilities
Carries shared state for AST-to-AST rewriting: original source text, fresh-name generation, and a mutable lexical scope stack.

## Datatypes
- `ScopeFrame`: snapshot of the current lexical scope kind, async-function status, and declared `global`/`nonlocal` names.
- `Context`: pass context with source text and interior-mutable stack of `ScopeFrame`s.

## Functions
- `ScopeFrame::module`: creates the root module scope.
- `ScopeFrame::new`: creates a non-root scope frame.
- `Context::new`: initializes context for a source string.
- `Context::line_number_at`: converts a byte offset into a one-based line number.
- `Context::fresh`: returns a fresh generated name with the requested prefix.
- `Context::push_scope` / `pop_scope`: maintain the current lexical-scope stack.
- `Context::current_scope`: returns the top scope frame, defaulting to module if the stack is empty.

## Context Read
- `soac-blockpy/src/passes/ast_to_ast/scope_helpers.rs` for `ScopeKind` and generated-name conventions.
- `soac-blockpy/src/namegen.rs` for fresh-name behavior.
