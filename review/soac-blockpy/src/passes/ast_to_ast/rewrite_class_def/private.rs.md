# soac-blockpy/src/passes/ast_to_ast/rewrite_class_def/private.rs

## File Responsibilities
Applies Python private-name mangling inside class bodies for identifiers and attributes that begin with `__` but do not end with `__`.

## Datatypes
- `PrivateRewriter`: transformer configured with the current class name, responsible for deciding and applying mangling.

## Functions
- `rewrite_private_names`: entry point that runs private-name rewriting over a class body.
- `PrivateRewriter::maybe_mangle`: returns the mangled spelling for a private name when Python rules apply.
- `PrivateRewriter::mangle_identifier` / `mangle_name`: mutate Ruff identifiers/name wrappers in place.
- `PrivateRewriter::visit_stmt`: avoids rewriting nested class/function scopes incorrectly and rewrites visible class-body statements.
- `PrivateRewriter::visit_parameter`: mangles parameter names where class-body rules require it.
- `PrivateRewriter::visit_expr`: rewrites name and attribute expressions.

## Context Read
- `soac-blockpy/src/passes/ast_to_ast/rewrite_class_def/mod.rs` for where private rewriting is invoked.
- Ruff AST identifier/name structures.
