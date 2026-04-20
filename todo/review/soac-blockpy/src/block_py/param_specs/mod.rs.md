# soac-blockpy/src/block_py/param_specs/mod.rs

## File Responsibilities

Represents Python callable parameter layout in BlockPy and extracts that layout plus default
expressions from Ruff `Parameters`.

## Datatypes

- `ParamKind`: Python parameter category: ordinary positional-or-keyword, positional-only,
  varargs, keyword-only, or kwargs.
- `Param`: one parameter name, kind, and whether it has a default.
- `ParamDefaultSource<'a>`: maps a parameter default to either a positional-default tuple index or
  keyword-only default name.
- `ParamSpec`: ordered parameter list for a function.

## Functions

- `ParamSpec::len`, `iter`: basic parameter list access.
- `ParamSpec::iter_with_default_sources`: iterates params with default-source metadata used by
  function construction and argument binding.
- `ParamSpec::names`: returns ordered names.
- `ParamSpec::default_count`: counts parameters with defaults.
- `ParamSpec::positional_param_indices`: returns indices for positional-only and ordinary params.
- `ParamSpec::param_index`, `vararg_index`, `kwarg_index`: find parameters by role/name.
- `ParamSpec::validate_default_count`: asserts that collected defaults match `has_default` flags.
- `push_param`: internal helper that appends a `Param` and its default expression.
- `collect_param_spec_and_defaults`: converts Ruff parameters into `ParamSpec` plus ordered default
  expressions.
- `param_defaults_to_expr`: packs defaults into a runtime tuple expression.

## Context Read

- `soac-blockpy/src/passes/ast_to_ast/expr_utils.rs`
- `soac-blockpy/src/block_py/mod.rs`
