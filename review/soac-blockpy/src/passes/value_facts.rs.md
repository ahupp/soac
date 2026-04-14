# soac-blockpy/src/passes/value_facts.rs

## File Responsibilities
Infers lightweight facts about codegen-stage expression values and local environments. Facts describe Python object type/singleton/refcount/provenance, integer/truthiness values, runtime helper signatures, and branch-sensitive environment narrowing.

## Datatypes
- `TruthinessFact`: known truthiness state.
- `PyExactType`, `TypeFact`, `RuntimeSingleton`, `NoneFact`, `BoolSingletonFact`, `RefcountFact`, and `ProvenanceFact`: Python-object fact dimensions.
- `RuntimeHelperId`, `RuntimeHelperSignature`, and `ThrowSpec`: known runtime helper identity, result type, and exception behavior.
- `CallableFact`: known callable identity/facts.
- `PyObjFacts`: aggregate facts for a Python object value.
- `I32Facts`, `I64Facts`, and `BoolFacts`: scalar value facts.
- `ValueFacts`: sum type for PyObject, i32, i64, bool, or unknown facts.
- `EnvFacts`: local-environment facts at a CFG point.
- `FactStore`: final fact database for expression keys and block entries.
- `FunctionFactInferer`: per-function dataflow engine.

## Functions
- `RuntimeHelperId::from_runtime_symbol`: maps runtime symbol names to known helper ids.
- `PyObjFacts` methods construct, combine, and query exact type, singleton, truthiness, refcount, provenance, callable, and helper-result facts.
- `none_fact_for_exact_type`: derives none-ness from an exact type.
- `ValueFacts` methods query and convert aggregate facts.
- `runtime_helper_result` / `runtime_helper_throw_spec`: declare facts for known runtime helper calls.
- `EnvFacts` methods query, set, remove, and intersect local PyObject facts.
- `FactStore` methods query facts by instruction key or block entry and iterate stored facts.
- `FunctionFactInferer::infer_expr_facts`, `infer_expr_facts_in_env`, `transfer_block_env`, `transfer_instr_env`, `successor_envs`, `infer_if_edge_facts`, `infer_block_entry_facts`, and `infer_branch_local_fact`: implement expression and CFG dataflow inference.
- `infer_function_value_facts`: computes facts for one function.
- Branch/singleton helpers `infer_local_is_singleton_comparison`, `expr_singleton_branch_facts`, and `local_load_location` narrow local facts across branches.
- Literal/runtime helpers `infer_runtime_name_load_facts`, `module_constant_load_fact`, `infer_module_constant_facts`, `infer_literal_facts`, and `truthiness` assign facts to leaves.
- `infer_module_value_facts`: public module-wide fact inference entry point.

## Context Read
- `soac-blockpy/src/passes/mod.rs` for codegen instruction shapes.
- `soac-blockpy/src/block_py/literal.rs`, `scope.rs`, and metadata/id types for fact keys and values.
- `local_env_plan.rs` and `ownership_effects.rs` for fact consumers.
