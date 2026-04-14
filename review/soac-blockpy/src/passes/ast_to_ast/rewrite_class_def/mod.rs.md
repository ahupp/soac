# soac-blockpy/src/passes/ast_to_ast/rewrite_class_def/mod.rs

## File Responsibilities
Lowers Ruff class definitions into explicit namespace-function construction plus a call to the SOAC class creation helper. It preserves docstrings, decorators, type parameters, generic bases, class cells, private-name handling, and class-body semantic boundaries.

## Datatypes
- None defined directly; this module composes AST statements, expressions, and type-parameter info from adjacent modules.

## Functions
- `class_def_to_create_class_fn`: main lowering routine that turns a `StmtClassDef` into helper function statements and a `create_class` assignment.
- `make_generic_base`: builds a synthetic `typing.Generic[...]` base for type-parameterized classes when needed.
- `arguments_has_generic`: detects whether an existing class base list already includes `Generic`.
- `is_generic_expr`: recognizes expressions that refer to `Generic`.
- `class_call_arguments`: constructs the argument list passed to `__soac__.create_class`, including name, bases, namespace function, decorators, docstring, type params, and class-cell information.

## Context Read
- `class_body.rs`, `method.rs`, and `private.rs` for class-body, method, and name-mangling subpasses.
- `soac-blockpy/src/passes/ast_to_ast/body.rs` for docstring splitting.
- `soac-blockpy/src/passes/ast_to_ast/semantic/mod.rs` for semantic scope information.
