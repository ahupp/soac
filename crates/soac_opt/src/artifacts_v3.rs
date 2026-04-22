use crate::emit_v3::{MechanicalModuleEmission, validate_mechanical_emission_matches_plan_v3};
use crate::plan_v3::ModuleOptimizationPlanV3;
use anyhow::{Context, Result, anyhow, bail};
use soac_core::block_py::{SerializedFunctionId, SerializedModuleId};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

const OPTIMIZATION_ARTIFACTS_V3_FORMAT_VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExactIntBranchV3Artifacts {
    pub plan: ModuleOptimizationPlanV3,
    pub emission: MechanicalModuleEmission,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct OptimizationArtifactsV3File {
    pub format_version: u32,
    pub artifacts: ExactIntBranchV3Artifacts,
}

pub fn write_optimization_artifacts_v3(
    path: &Path,
    artifacts: &ExactIntBranchV3Artifacts,
) -> Result<()> {
    validate_optimization_artifacts_v3(artifacts)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create optimization plan v3 dir {}", parent.display()))?;
    }
    let file = OptimizationArtifactsV3File {
        format_version: OPTIMIZATION_ARTIFACTS_V3_FORMAT_VERSION,
        artifacts: artifacts.clone(),
    };
    let archive = rkyv::to_bytes::<rkyv::rancor::Error>(&file)
        .map_err(|err| anyhow!("serialize optimization plan v3: {err}"))?;
    let temp_path = path.with_extension("optv3.tmp");
    {
        let mut temp_file = File::create(temp_path.as_path()).with_context(|| {
            format!(
                "create temporary optimization plan v3 {}",
                temp_path.display()
            )
        })?;
        temp_file
            .write_all(archive.as_ref())
            .with_context(|| format!("write optimization plan v3 {}", temp_path.display()))?;
    }
    fs::rename(temp_path.as_path(), path).with_context(|| {
        format!(
            "publish optimization plan v3 {} -> {}",
            temp_path.display(),
            path.display()
        )
    })
}

pub fn load_optimization_artifacts_v3(path: &Path) -> Result<ExactIntBranchV3Artifacts> {
    let bytes =
        fs::read(path).with_context(|| format!("read optimization plan v3 {}", path.display()))?;
    let file =
        rkyv::from_bytes::<OptimizationArtifactsV3File, rkyv::rancor::Error>(bytes.as_slice())
            .map_err(|err| anyhow!("deserialize optimization plan v3 {}: {err}", path.display()))?;
    if file.format_version != OPTIMIZATION_ARTIFACTS_V3_FORMAT_VERSION {
        bail!(
            "optimization plan v3 {} has format version {}, expected {}",
            path.display(),
            file.format_version,
            OPTIMIZATION_ARTIFACTS_V3_FORMAT_VERSION
        );
    }
    validate_optimization_artifacts_v3(&file.artifacts)?;
    Ok(file.artifacts)
}

pub fn validate_optimization_artifacts_v3(artifacts: &ExactIntBranchV3Artifacts) -> Result<()> {
    validate_mechanical_emission_matches_plan_v3(&artifacts.plan, &artifacts.emission)
        .map_err(|err| anyhow!("validate optimization plan v3 artifacts: {err}"))
}

pub fn validate_optimization_artifacts_v3_for_module(
    artifacts: &ExactIntBranchV3Artifacts,
    module_name: &str,
    source_hash: u64,
    cache_identity: &str,
) -> Result<()> {
    validate_optimization_artifacts_v3(artifacts)?;
    if artifacts.plan.module.module_name != module_name {
        bail!(
            "optimization plan v3 module name is {}, expected {module_name}",
            artifacts.plan.module.module_name
        );
    }
    if artifacts.plan.module.source_hash != source_hash {
        bail!(
            "optimization plan v3 source hash for module {module_name} is 0x{:016x}, expected 0x{source_hash:016x}",
            artifacts.plan.module.source_hash
        );
    }
    if artifacts.plan.module.cache_identity != cache_identity {
        bail!(
            "optimization plan v3 cache identity for module {module_name} is {}, expected {cache_identity}",
            artifacts.plan.module.cache_identity
        );
    }
    let current_identity = artifacts
        .plan
        .identity_tables
        .module(SerializedModuleId::new(0))
        .map_err(|err| anyhow!("optimization plan v3 identity table: {err}"))?;
    if current_identity.module_name != module_name {
        bail!(
            "optimization plan v3 identity table module 0 is {}, expected {module_name}",
            current_identity.module_name
        );
    }
    if current_identity.source_hash != source_hash {
        bail!(
            "optimization plan v3 identity table source hash for module 0 is 0x{:016x}, expected 0x{source_hash:016x}",
            current_identity.source_hash
        );
    }
    if current_identity.cache_identity.as_deref() != Some(cache_identity) {
        bail!(
            "optimization plan v3 identity table cache identity for module 0 is {:?}, expected {cache_identity}",
            current_identity.cache_identity
        );
    }
    if artifacts.emission.module_name != module_name {
        bail!(
            "optimization plan v3 emission module name is {}, expected {module_name}",
            artifacts.emission.module_name
        );
    }
    Ok(())
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
