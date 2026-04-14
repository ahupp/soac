# soac-blockpy/src/passes/ruff_to_blockpy/module_plan/mod.rs

## File Responsibilities

Rewrites the simplified Ruff AST module into a `BlockPyModule`. It extracts every function/lambda
into a callable definition, replaces function definitions with `__soac__.make_function(...)`
instantiations, preserves decorators/defaults/annotation helpers, and lowers the root module-init
function.

## Datatypes

- `FunctionScopeFrame`: stack frame for the current semantic scope, current callable scope, and
  statements hoisted to the parent scope.
- `PendingAnnotationHelper`: saved make-function expression for a generated function-annotation
  helper that must be attached to its target function.
- `BlockPyModuleRewriter<'a, P>`: AST transformer that owns module lowering state and accumulated
  callable definitions.
- `YieldFamilyDetector`: visitor flagging whether a function body contains yield/yield-from outside
  nested callables/comprehensions.

## Functions

- `rewrite_ast_to_core_blockpy_module_plan_with_module`: flattens the AST, lowers the root
  `_dp_module_init`, and returns a `BlockPyModule`.
- `YieldFamilyDetector::visit_stmt`, `visit_expr`: scan only the current callable for yield-family
  expressions.
- `function_kind`: classifies functions as function, generator, coroutine, or async generator.
- `try_lower_function_to_core_blockpy_bundle`: lowers one function body to a core BlockPy function.
- `build_lowered_function_instantiation_expr`: builds the `__soac__.make_function(...)` expression
  and applies decorators.
- `rewrite_function_def_stmt_via_blockpy_with_pass`: lowers a function definition, creates its
  instantiation statements, handles type params, stores the callable def, and hoists nested setup.
- `BlockPyModuleRewriter::visit_function_definition_exprs`: visits decorators and default values
  without walking the function body.
- `BlockPyModuleRewriter::lower_lambda_expr`: turns a lambda into a synthetic function definition,
  lowers it, and returns a make-function expression.
- `BlockPyModuleRewriter::root_module_init_stmt`: validates and returns the synthetic root module
  init function.
- `BlockPyModuleRewriter::walk_function_def_with_scope`: fetches semantic scope and walks a
  function body under it.
- `BlockPyModuleRewriter::walk_function_def_with_explicit_scope`: pushes scope state, visits a
  function body, and returns the resulting frame.
- `BlockPyModuleRewriter::lower_root_function_def`: lowers the root module function.
- `BlockPyModuleRewriter::rewrite_visited_function_def`: consumes annotation helper info and
  rewrites a nested function definition into instantiation statements.
- `BlockPyModuleRewriter::pending_annotation_helper_target_name`: detects synthetic annotation
  helper names.
- `BlockPyModuleRewriter::lower_pending_annotation_helper`: lowers and records a generated
  annotation helper function.
- `BlockPyModuleRewriter::consume_pending_annotation_helper`: returns the make-function expression
  for a target's pending annotation helper, or `None`.
- `BlockPyModuleRewriter` `Transformer` methods: rewrite bodies statement-by-statement, lower
  nested function definitions, and replace lambda expressions.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/module_plan/callable_scope.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/mod.rs`
- `soac-blockpy/src/block_py/param_specs/mod.rs`
- `soac-blockpy/src/passes/ast_to_ast/semantic/mod.rs`

