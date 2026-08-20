pub mod access_emission_v3;
pub mod alternatives_v3;
pub mod artifacts_v3;
pub mod call_emission_v3;
pub mod evidence_v3;
pub mod operator_specialization;
pub mod passes;
pub mod pipeline_v3;
pub mod plan;
pub mod planner_v3;
pub mod region_emission_v3;
pub mod region_v3;
mod typed;
pub mod v3_status;

/// Shared activation-safety precondition for inlining and its cost planning.
pub use passes::inline_callee_preserves_activation;
