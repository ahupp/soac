# crates/soac_jit/src/jit/planning.rs

## File Responsibilities

Adapts BlockPy local-environment, refcount, and deopt-resume plans into JIT-specific plans that codegen can consume. It
validates that planned runtime block parameters, stack-slot materializations, edge transports, exception dispatch, and deopt
points match the current JIT lowering model, and provides renderers for inspection.

## Datatypes

- Re-exported pass datatypes: `BlockParamFacts`, `FunctionLocalPlan`, `LocalRefKind`, `ParamBindingFacts`,
  `ParamProvenance`, `PlannedLocalBinding`, `PlannedLocalStorage`, and related helpers are surfaced for JIT consumers.
- `CurrentJitRefcountPlanCheck`: summary counters from validating ownership effects against the current JIT cleanup model.
- `BlockExcDispatchPlan`: how an exception edge writes stack slots, passes target args, forwards locals, and sinks ownership.
- `EdgeTransportPlan`: how a normal edge moves values by slot writes, runtime target args, and forwarded locals.
- `RuntimeBlockParamPlan`: mapping from a runtime block argument to a planned local binding plus entry aliases.
- `PlannedStackSlotEntrySeed`: stack-slot local that must be loaded into the local environment at block entry.
- `PlannedLocalEnvEntrySource`: whether an entry materialization comes from a block param or stack-slot load.
- `PlannedLocalEnvEntryMaterialization`: one block-entry local materialization, including source and reference kind.
- `PlannedJitFunctionLocals`: full per-function JIT plan: local plan, refcount plan, runtime params, transports,
  stack-slot seeds, entry materializations, and exception dispatches.
- `PlannedJitModuleLocals`: module map from `FunctionId` to per-function local plans.
- `PlannedJitDeoptResumeFunction`: per-function local-env resume plan plus generated JIT deopt-point ids.
- `PlannedJitDeoptResumeModule`: module map from `FunctionId` to deopt-resume plans.
- `PlannedJitDeoptPointId`: stable per-function ordinal id for a deopt point.
- `PlannedJitDeoptPoint`: codegen-facing deopt point with resume point, precision, and materialized local locations.

## Functions

- Refcount validation: `can_release_via_stack_slot_fallback`, `plan_function_refcount_ownership`,
  `CurrentJitRefcountPlanCheck::has_edge_release_gaps`, `check_refcount_plan_against_current_jit`, and
  `check_local_has_storage_layout_entry` adapt and check ownership actions against JIT-supported locals.
- Plan validation methods: `PlannedJitModuleLocals::function`, `PlannedJitModuleLocals::validate_for_module`,
  `PlannedJitDeoptResumeFunction::{entry,deopt_point,deopt_point_by_id,deopt_points_for_block,validate_for_function}`,
  `PlannedJitDeoptResumeModule::{function,entry,deopt_point,validate_for_module}`, and
  `PlannedJitFunctionLocals::{required_stack_slot_names_for_function,validate_for_function}` check structural consistency.
- Exception/materialization validators: `validate_exception_dispatch_ownership_sinks`, `named_block_arg_sources`, and
  `validate_entry_materializations_for_block` ensure forwarded values have exactly one ownership sink and materializations
  match their sources.
- Module/function planning: `plan_jit_module_locals`, `plan_jit_module_locals_from_passes`,
  `plan_jit_function_locals`, and `plan_jit_function_locals_from_plans` build local/refcount plans from pass outputs.
- Deopt planning: `plan_jit_deopt_resume_module`, `plan_jit_deopt_resume_module_from_passes`, and
  `planned_deopt_points_from_resume_plan` derive deopt-resume metadata.
- Renderers: `render_jit_deopt_resume_module`, `render_jit_deopt_resume_function`, `render_jit_deopt_point`,
  `render_jit_module_locals`, `render_jit_function_locals`, `render_local_env_entry_materialization`,
  `render_edge_transport`, and `render_named_block_args` produce inspection text.
- Entry/edge planning helpers: `local_ref_kind_for_stack_mirror`, `planned_jit_params_for_function`,
  `planned_stack_slot_entry_seeds_for_function`, `planned_local_env_entry_materializations_for_function`,
  `plan_edge_transport`, `planned_implicit_target_transports_for_function`, `planned_jump_edge_transports_for_function`,
  `exc_dispatch_plan`, and `planned_drop_forwarded_local_names` build codegen movement/materialization plans.

Tests cover local/refcount/deopt planning shapes for representative control-flow, ownership, and exception cases.

## Context Read

- `soac_blockpy::passes`: source local-env, refcount, fact, and deopt-resume planning APIs.
- `soac_blockpy::block_py`: BlockPy function, block, edge, term, and argument datatypes consumed by the planner.
- `crates/soac_jit/src/jit/mod.rs`: consumes the planned locals, transports, cleanup, and deopt-resume metadata during codegen.
