use crate::emit_v3::MechanicalModuleEmission;
use crate::plan_v3::ModuleOptimizationPlanV3;
use anyhow::{Context, Result, anyhow, bail};
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
    Ok(file.artifacts)
}
