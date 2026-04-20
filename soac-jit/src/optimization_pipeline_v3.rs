use crate::optimization_alternatives_v3::AlternativeCatalog;
use crate::optimization_emit_v3::{
    MechanicalEmitError, MechanicalModuleEmission, emit_mechanical_plan_v3,
};
use crate::optimization_evidence_v3::{
    PlannerFactHints, planner_fact_hints_from_module_constants_v3,
    planner_facts_from_profile_evidence_v3,
};
use crate::optimization_plan::{
    CachedModuleOptimizationInput, FunctionProfileEvidence, ModuleOptimizationPlanReport,
    OptimizationPlanGenerationSummary, ProfileEvidenceStore, cached_module_paths_under_root,
};
use crate::optimization_plan_v3::{
    FunctionPlanIdentity, ModuleOptimizationPlanV3, ModulePlanIdentity, PlanDiagnostic, RegionId,
};
use crate::optimization_planner_v3::{
    ExtractedRegionPlanRequest, FunctionPlanRequest, ModulePlanRequest, plan_module_optimization_v3,
};
use crate::optimization_region_v3::{
    RegionExtractionAttempt, RegionExtractionError, extract_function_regions_v3,
};
use anyhow::{Context, Result, anyhow, bail};
use soac_core::block_py::{
    BlockLabel, BlockPyFunction, BlockPyModule, LocalFunctionId, SerializedFunctionId,
    SerializedModuleId,
};
use soac_lowering::codegen_cache::{
    CachedCodegenModule, CachedCodegenModuleMetadata, load_codegen_module_cache,
    module_optimization_plan_v3_path,
};
use soac_lowering::passes::CodegenModuleShape;
use soac_lowering::passes::InstrResolved;
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

const OPTIMIZATION_ARTIFACTS_V3_FORMAT_VERSION: u32 = 1;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExactIntBranchV3Error {
    Emit(MechanicalEmitError),
}

impl fmt::Display for ExactIntBranchV3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Emit(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ExactIntBranchV3Error {}

pub fn plan_and_emit_function_exact_int_branches_v3(
    catalog: &AlternativeCatalog,
    module: ModulePlanIdentity,
    function: FunctionPlanIdentity,
    lowered_function: &BlockPyFunction<CodegenModuleShape>,
    evidence: &FunctionProfileEvidence,
    hints_by_region: &HashMap<RegionId, PlannerFactHints>,
) -> Result<ExactIntBranchV3Artifacts, ExactIntBranchV3Error> {
    let attempts = extract_function_regions_v3(lowered_function);
    plan_and_emit_extracted_exact_int_branches_v3(
        catalog,
        module,
        function,
        attempts,
        evidence,
        hints_by_region,
    )
}

pub fn plan_and_emit_function_exact_int_branches_v3_with_module_constants(
    catalog: &AlternativeCatalog,
    module: ModulePlanIdentity,
    function: FunctionPlanIdentity,
    lowered_function: &BlockPyFunction<CodegenModuleShape>,
    evidence: &FunctionProfileEvidence,
    module_constants: &[InstrResolved],
) -> Result<ExactIntBranchV3Artifacts, ExactIntBranchV3Error> {
    let attempts = extract_function_regions_v3(lowered_function);
    let hints_by_region = attempts
        .iter()
        .filter_map(|attempt| {
            let region = attempt.result.as_ref().ok()?;
            Some((
                region.id,
                planner_fact_hints_from_module_constants_v3(region, module_constants),
            ))
        })
        .collect::<HashMap<_, _>>();
    plan_and_emit_extracted_exact_int_branches_v3(
        catalog,
        module,
        function,
        attempts,
        evidence,
        &hints_by_region,
    )
}

pub fn plan_and_emit_module_v3_from_raw_evidence(
    catalog: &AlternativeCatalog,
    metadata: &CachedCodegenModuleMetadata,
    lowered_module: &BlockPyModule<CodegenModuleShape>,
    evidence_store: &ProfileEvidenceStore,
) -> Result<ExactIntBranchV3Artifacts, ExactIntBranchV3Error> {
    let module = ModulePlanIdentity {
        module_name: metadata.module_name.clone(),
        source_hash: metadata.source_hash,
        cache_identity: metadata.cache_identity.clone(),
    };
    let mut functions = Vec::new();
    let mut diagnostics_by_function = Vec::new();
    for function in &lowered_module.callable_defs {
        let attempts = extract_function_regions_v3(function);
        let hints_by_region = attempts
            .iter()
            .filter_map(|attempt| {
                let region = attempt.result.as_ref().ok()?;
                Some((
                    region.id,
                    planner_fact_hints_from_module_constants_v3(
                        region,
                        lowered_module.module_constants.as_slice(),
                    ),
                ))
            })
            .collect::<HashMap<_, _>>();
        let evidence = evidence_store.evidence_for_runtime_function_v3(
            metadata.module_name.as_str(),
            metadata.source_hash,
            function.function_id,
        );
        let mut region_requests = Vec::new();
        let mut diagnostics = Vec::new();
        for attempt in attempts {
            match attempt.result {
                Ok(region) => {
                    let hints = hints_by_region.get(&region.id).cloned().unwrap_or_default();
                    let facts = planner_facts_from_profile_evidence_v3(&region, &evidence, &hints);
                    region_requests.push(ExtractedRegionPlanRequest { region, facts });
                }
                Err(error) => diagnostics.push(extraction_diagnostic(attempt.block, error)),
            }
        }
        functions.push(FunctionPlanRequest {
            function: function_plan_identity_v3(function),
            regions: region_requests,
        });
        diagnostics_by_function.push(diagnostics);
    }

    let mut plan = plan_module_optimization_v3(catalog, ModulePlanRequest { module, functions });
    for (function, diagnostics) in plan.functions.iter_mut().zip(diagnostics_by_function) {
        function.diagnostics.extend(diagnostics);
    }
    let emission = emit_mechanical_plan_v3(&plan).map_err(ExactIntBranchV3Error::Emit)?;
    Ok(ExactIntBranchV3Artifacts { plan, emission })
}

pub fn plan_and_emit_extracted_exact_int_branches_v3(
    catalog: &AlternativeCatalog,
    module: ModulePlanIdentity,
    function: FunctionPlanIdentity,
    attempts: Vec<RegionExtractionAttempt>,
    evidence: &FunctionProfileEvidence,
    hints_by_region: &HashMap<RegionId, PlannerFactHints>,
) -> Result<ExactIntBranchV3Artifacts, ExactIntBranchV3Error> {
    let mut region_requests = Vec::new();
    let mut diagnostics = Vec::new();
    for attempt in attempts {
        match attempt.result {
            Ok(region) => {
                let hints = hints_by_region.get(&region.id).cloned().unwrap_or_default();
                let facts = planner_facts_from_profile_evidence_v3(&region, evidence, &hints);
                region_requests.push(ExtractedRegionPlanRequest { region, facts });
            }
            Err(error) => diagnostics.push(extraction_diagnostic(attempt.block, error)),
        }
    }

    let mut plan = plan_module_optimization_v3(
        catalog,
        ModulePlanRequest {
            module,
            functions: vec![FunctionPlanRequest {
                function,
                regions: region_requests,
            }],
        },
    );
    if let Some(function) = plan.functions.first_mut() {
        function.diagnostics.extend(diagnostics);
    }
    let emission = emit_mechanical_plan_v3(&plan).map_err(ExactIntBranchV3Error::Emit)?;
    Ok(ExactIntBranchV3Artifacts { plan, emission })
}

fn function_plan_identity_v3(
    function: &BlockPyFunction<CodegenModuleShape>,
) -> FunctionPlanIdentity {
    FunctionPlanIdentity {
        function: SerializedFunctionId::new(
            SerializedModuleId::new(0),
            LocalFunctionId::new(function.function_id.local_function_id().as_u32()),
        ),
        debug_name: Some(function.names.qualname.clone()),
    }
}

fn extraction_diagnostic(block: BlockLabel, error: RegionExtractionError) -> PlanDiagnostic {
    PlanDiagnostic {
        source: None,
        message: format!("v3 extraction declined block {block}: {error}"),
    }
}

pub fn generate_optimization_plans_v3_for_cached_modules(
    evidence_store: &ProfileEvidenceStore,
    module_inputs: impl IntoIterator<Item = CachedModuleOptimizationInput>,
    out_root: &Path,
) -> Result<OptimizationPlanGenerationSummary> {
    let mut summary = OptimizationPlanGenerationSummary::default();
    for module_input in module_inputs {
        match generate_module_optimization_plan_v3(
            evidence_store,
            module_input.module_path.as_path(),
            out_root,
            module_input.strict,
        )? {
            Some(report) => summary.reports.push(report),
            None => summary.skipped += 1,
        }
    }
    Ok(summary)
}

pub fn generate_optimization_plans_v3_for_counter_dump(
    counters_path: &Path,
    module_root: &Path,
    out_root: &Path,
) -> Result<OptimizationPlanGenerationSummary> {
    let evidence_store = ProfileEvidenceStore::from_counter_dump(counters_path)?;
    let module_inputs = cached_module_paths_under_root(module_root)?
        .into_iter()
        .map(|module_path| CachedModuleOptimizationInput::new(module_path, false));
    generate_optimization_plans_v3_for_cached_modules(&evidence_store, module_inputs, out_root)
}

pub fn generate_module_optimization_plan_v3(
    evidence_store: &ProfileEvidenceStore,
    module_path: &Path,
    out_root: &Path,
    strict: bool,
) -> Result<Option<ModuleOptimizationPlanReport>> {
    let cache = load_codegen_module_cache(module_path)
        .with_context(|| format!("load BlockPy module cache {}", module_path.display()))?;
    if !counter_evidence_matches_cached_module_v3(evidence_store, &cache, strict)? {
        return Ok(None);
    }
    let catalog = AlternativeCatalog::default_v3();
    let artifacts = plan_and_emit_module_v3_from_raw_evidence(
        &catalog,
        &cache.metadata,
        &cache.module,
        evidence_store,
    )
    .map_err(|err| anyhow!("generate optimizer v3 plan: {err}"))?;
    let output_path = module_optimization_plan_v3_path(
        out_root,
        cache.metadata.source,
        cache.metadata.module_name.as_str(),
    )
    .with_context(|| {
        format!(
            "construct optimization plan v3 output path for module {}",
            cache.metadata.module_name
        )
    })?;
    write_optimization_artifacts_v3(output_path.as_path(), &artifacts)?;
    Ok(Some(ModuleOptimizationPlanReport {
        output_path,
        module_name: cache.metadata.module_name,
        source_hash: cache.metadata.source_hash,
        function_count: artifacts.plan.functions.len(),
    }))
}

fn counter_evidence_matches_cached_module_v3(
    evidence_store: &ProfileEvidenceStore,
    cache: &CachedCodegenModule,
    strict: bool,
) -> Result<bool> {
    match evidence_store.module_source_hash(cache.metadata.module_name.as_str()) {
        Some(source_hash) if source_hash == cache.metadata.source_hash => Ok(true),
        Some(source_hash) => bail!(
            "counter dump source hash for module {} is 0x{source_hash:016x}, but cached BlockPy module has 0x{:016x}",
            cache.metadata.module_name,
            cache.metadata.source_hash
        ),
        None if strict => bail!(
            "counter dump does not contain module {}",
            cache.metadata.module_name
        ),
        None => Ok(false),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_specialization::{ExactTypeTag, pack_binary_shape};
    use crate::optimization_plan_v3::{RegionId, validate_module_plan_v3};
    use crate::optimization_region_v3::{ExtractedValueId, extract_block_region_v3};
    use soac_core::block_py::{
        BinOp, BinOpKind, Block, BlockLabel, BlockParam, BlockPyName, BlockTerm, InstrId, Load,
        LocalFunctionId, LocalLocation, Meta, NameLocation, ResolvedName, SerializedFunctionId,
        SerializedModuleId, TermIf, WithMeta,
    };
    use soac_lowering::passes::InstrCodegen;

    fn label(index: usize) -> BlockLabel {
        BlockLabel::from_index(index)
    }

    fn instr_id(index: u32) -> InstrId {
        InstrId::new(label(0), index)
    }

    fn with_instr_id(instr: InstrCodegen, index: u32) -> InstrCodegen {
        instr.with_meta(Meta {
            instr_id: Some(instr_id(index)),
            ..Meta::synthetic()
        })
    }

    fn local(name: &str, slot: u32) -> InstrCodegen {
        InstrCodegen::Load(Load::new(ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Local(LocalLocation(slot)),
        }))
    }

    fn binary(op: BinOpKind, left: InstrCodegen, right: InstrCodegen, id: u32) -> InstrCodegen {
        with_instr_id(InstrCodegen::BinOp(BinOp::new(op, left, right)), id)
    }

    fn branch_block() -> Block<InstrCodegen> {
        let add = binary(
            BinOpKind::Add,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let test = binary(BinOpKind::Gt, add, with_instr_id(local("zero", 2), 3), 4);
        Block::new(
            label(0),
            Vec::new(),
            BlockTerm::IfTerm(TermIf {
                test,
                then_label: label(1),
                else_label: label(2),
            }),
            Vec::<BlockParam>::new(),
            None,
        )
    }

    fn module_identity() -> ModulePlanIdentity {
        ModulePlanIdentity {
            module_name: "pkg.mod".to_string(),
            source_hash: 0x99,
            cache_identity: "test-cache".to_string(),
        }
    }

    fn function_identity() -> FunctionPlanIdentity {
        FunctionPlanIdentity {
            function: SerializedFunctionId::new(
                SerializedModuleId::new(0),
                LocalFunctionId::new(1),
            ),
            debug_name: Some("f".to_string()),
        }
    }

    fn evidence() -> FunctionProfileEvidence {
        let mut evidence = FunctionProfileEvidence::default();
        evidence.operator_specializations.insert(
            instr_id(2),
            vec![pack_binary_shape(ExactTypeTag::Int, ExactTypeTag::Int)],
        );
        evidence
    }

    fn hints_by_region() -> HashMap<RegionId, PlannerFactHints> {
        let mut hints = PlannerFactHints::default();
        hints.set_i64_constant(ExtractedValueId(3), 0);
        HashMap::from([(RegionId(0), hints)])
    }

    #[test]
    fn routes_exact_int_branch_through_v3_plan_and_emitter() {
        let region = extract_block_region_v3(&branch_block(), RegionId(0)).unwrap();
        let artifacts = plan_and_emit_extracted_exact_int_branches_v3(
            &AlternativeCatalog::default_v3(),
            module_identity(),
            function_identity(),
            vec![RegionExtractionAttempt {
                block: label(0),
                result: Ok(region),
            }],
            &evidence(),
            &hints_by_region(),
        )
        .unwrap();

        validate_module_plan_v3(&artifacts.plan).unwrap();
        assert_eq!(artifacts.plan.functions[0].regions.len(), 2);
        assert_eq!(artifacts.emission.functions[0].regions.len(), 2);
        assert!(artifacts.plan.functions[0].diagnostics.is_empty());
    }

    #[test]
    fn extraction_declines_are_reported_as_plan_diagnostics() {
        let artifacts = plan_and_emit_extracted_exact_int_branches_v3(
            &AlternativeCatalog::default_v3(),
            module_identity(),
            function_identity(),
            vec![RegionExtractionAttempt {
                block: label(0),
                result: Err(RegionExtractionError::UnsupportedTerm {
                    block: label(0),
                    term: "Jump",
                }),
            }],
            &FunctionProfileEvidence::default(),
            &HashMap::new(),
        )
        .unwrap();

        assert!(artifacts.plan.functions[0].regions.is_empty());
        assert_eq!(artifacts.plan.functions[0].diagnostics.len(), 1);
        assert!(
            artifacts.plan.functions[0].diagnostics[0]
                .message
                .contains("v3 extraction declined block")
        );
        assert!(artifacts.emission.functions[0].regions.is_empty());
    }
}
