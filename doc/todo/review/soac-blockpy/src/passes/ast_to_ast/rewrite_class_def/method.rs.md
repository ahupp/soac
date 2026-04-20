# soac-blockpy/src/passes/ast_to_ast/rewrite_class_def/method.rs

## File Responsibilities
Handles method-specific class rewrite behavior around zero-argument and explicit `super()` use, especially when methods need access to the synthetic `__class__` cell.

## Datatypes
- `MethodRewriteSuperClasscell`: transformer that rewrites explicit class-cell uses inside methods.
- `MethodExplicitSuperRewriter`: transformer that rewrites explicit `super(cls, self)` patterns when possible.
- `FunctionUsesClassCellDetector`: transformer that detects whether a function body references the implicit class cell.

## Functions
- `MethodRewriteSuperClasscell::visit_stmt` / `visit_expr`: rewrite method statements and expressions while avoiding nested scope mistakes.
- `is_dp_call`: recognizes SOAC helper calls by name.
- `is_super_call`: recognizes `super(...)` call expressions.
- `rewrite_explicit_super_classcell`: class-level entry point for explicit-super class-cell rewriting.
- `MethodExplicitSuperRewriter::visit_stmt`: applies method rewrites to function definitions.
- `rewrite_method`: rewrites one function body and reports whether it needed class-cell support.
- `FunctionUsesClassCellDetector::visit_stmt` / `visit_expr`: detect `super()` or `__class__` use inside a function without crossing nested definitions.
- `function_uses_class_cell`: returns whether a function body needs the class cell.

## Context Read
- `soac-blockpy/src/passes/ast_to_ast/rewrite_class_def/private.rs` for adjacent class-body transformations.
- `soac-blockpy/src/passes/ast_to_ast/util.rs` for SOAC helper recognition.
