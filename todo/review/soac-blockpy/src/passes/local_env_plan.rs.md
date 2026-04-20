# soac-blockpy/src/passes/local_env_plan.rs

## File Responsibilities
Plans how source locals and block parameters map to local-environment storage for codegen and deoptimization/resume. It computes per-block bindings, reference-kind facts, resume snapshots before block/instruction/term points, validates plan coverage, and renders plans for diagnostics.

## Datatypes
- `LocalRefKind`: whether a local value is owned, borrowed, immortal, or deleted/unknown for refcount handling.
- `PlannedLocalStorage`: direct-local or environment-backed storage decision.
- `ParamBindingFacts` and `ParamProvenance`: facts about block parameter values and where they came from.
- `BlockParamFacts`: facts attached to a block parameter.
- `PlannedLocalBinding`: planned binding for one name in one block.
- `BlockLocalPlan`, `FunctionLocalPlan`, and `LocalEnvModulePlan`: per-block/function/module local storage plans.
- `LocalEnvResumeStatePrecision`, `LocalEnvResumePoint`, `LocalEnvResumeEntry`, `LocalEnvResumeBindingState`, `LocalEnvResumeValueSource`, `LocalEnvResumeBinding`, `FunctionLocalEnvResumePlan`, and `LocalEnvResumeModulePlan`: deopt/resume snapshot model for local values.

## Functions
- Plan accessors on `BlockLocalPlan`, `FunctionLocalPlan`, `LocalEnvModulePlan`, `LocalEnvResumeEntry`, `FunctionLocalEnvResumePlan`, and `LocalEnvResumeModulePlan` query bindings and resume entries.
- `validate_for_module` methods delegate to module/function validators.
- `LocalEnvResumePoint` constructors/accessors build and inspect block-entry, before-instruction, and before-terminator points.
- `plan_local_env_module`: computes local-env plans for every function.
- `plan_local_env_resume_module`: computes resume plans for every function using value facts.
- `plan_function_local_env_resume` and `plan_function_local_env_resume_with_deleted_constants`: build resume entries for one function.
- `validate_local_env_module_plan` / `validate_local_env_resume_module_plan`: verify plan coverage and consistency.
- `plan_function_locals`: computes per-block local bindings and storage kinds.
- Renderers `render_local_env_module_plan`, `render_local_env_resume_module_plan`, `render_local_env_resume_function_plan`, `render_local_env_function_plan`, `render_planned_local_binding`, and private render helpers produce readable diagnostics.
- Resume transfer helpers `resume_binding_from_planned_local`, `resume_binding_state_for_planned_local`, `resume_value_source_for_planned_local`, and `transfer_resume_local_state` model local state changes across instructions and terms.
- Fact helpers `expr_is_deleted_sentinel`, `deleted_sentinel_constant_slots`, and `local_ref_kind_for_resume_value` classify resume values.
- Validation/storage helpers `validate_function_local_plan`, `local_ref_kind_for_block_entry`, and `is_try_exception_alias_name` check edge cases.

## Context Read
- `value_facts.rs` for facts used to classify local values.
- `ownership_effects.rs` for related refcount/liveness analysis.
- `soac-blockpy/src/block_py/scope.rs` and CFG definitions for storage layout and block params.
