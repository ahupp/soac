# soac-blockpy/src/driver.rs

## File Responsibilities

Owns the end-to-end lowering pipeline from Python source to codegen-ready BlockPy: parse, AST
rewrites, module/class/function restructuring, control-flow lowering, await/yield lowering,
name/global/instruction-id passes, optional pre-optimization module cache, value/ownership/local-env
analysis, optional instrumentation, and validation.

## Datatypes

- `AstToAstPassResult`: AST-to-AST output plus semantic scope state needed by later lowering.
- `LoweringOptions`: caller options for treating runtime names as globals and using a
  pre-optimization cache path.

## Functions

- `AstToAstPassResult::pretty_print`: renders the rewritten AST as Python source.
- `rewrite_ast_to_ast_module`: applies private-name, annotation, string-template, helper-scope,
  semantic, module-init, and class-body rewrites.
- `rewrite_module_with_tracker_with_options`: public internal pipeline entry with options and env
  config.
- `rewrite_pre_optimization_module_with_cache`: loads/stores cached codegen BlockPy when a cache
  path is supplied, including function-id remapping on cache hits.
- `rewrite_pre_optimization_module_from_source`: runs parse through codegen instruction-id
  assignment without late analysis/instrumentation.
- `store_pre_optimization_cache`: writes cache and logs success/failure.
- `finish_codegen_module_with_tracker`: validates instruction ids, computes value facts, ownership
  and local-env plans, applies trace/call-target/locality/refcount instrumentation according to
  config, and validates the final module.
- `rewrite_module_with_tracker`: default-options entry point.
- `wrap_module_init`: wraps module statements in synthesized `_dp_module_init` and updates semantic
  scope state.

## Context Read

- `soac-blockpy/src/lib.rs`
- `soac-blockpy/src/env_config.rs`
- `soac-blockpy/src/codegen_cache.rs`
- `soac-blockpy/src/pass_tracker.rs`
- `soac-blockpy/src/passes/mod.rs`
