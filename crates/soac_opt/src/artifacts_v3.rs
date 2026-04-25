use anyhow::{Result, anyhow};
use soac_core::block_py::SerializedFunctionId;
use soac_ir_typed::emit_v3::{
    MechanicalModuleEmission, validate_mechanical_emission_matches_plan_v3,
};
use soac_ir_typed::plan_v3::ModuleOptimizationPlanV3;

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExactIntBranchV3Artifacts {
    pub plan: ModuleOptimizationPlanV3,
    pub emission: MechanicalModuleEmission,
}

pub fn validate_optimization_artifacts_v3(artifacts: &ExactIntBranchV3Artifacts) -> Result<()> {
    validate_mechanical_emission_matches_plan_v3(&artifacts.plan, &artifacts.emission)
        .map_err(|err| anyhow!("validate optimization plan v3 artifacts: {err}"))
}

pub fn single_function_optimization_artifacts_v3(
    artifacts: &ExactIntBranchV3Artifacts,
    function: SerializedFunctionId,
) -> Result<Option<ExactIntBranchV3Artifacts>> {
    validate_optimization_artifacts_v3(artifacts)?;
    let Some(planned_function) = artifacts
        .plan
        .functions
        .iter()
        .find(|planned| planned.function.function == function)
    else {
        return Ok(None);
    };
    let emitted_function = artifacts
        .emission
        .functions
        .iter()
        .find(|emitted| emitted.function == function)
        .ok_or_else(|| {
            anyhow!(
                "optimization plan v3 has planned function {} without matching mechanical emission",
                function
            )
        })?;
    Ok(Some(ExactIntBranchV3Artifacts {
        plan: ModuleOptimizationPlanV3 {
            module: artifacts.plan.module.clone(),
            identity_tables: artifacts.plan.identity_tables.clone(),
            helper_catalog_version: artifacts.plan.helper_catalog_version,
            cost_model_version: artifacts.plan.cost_model_version,
            functions: vec![planned_function.clone()],
        },
        emission: MechanicalModuleEmission {
            module_name: artifacts.emission.module_name.clone(),
            functions: vec![emitted_function.clone()],
        },
    }))
}
