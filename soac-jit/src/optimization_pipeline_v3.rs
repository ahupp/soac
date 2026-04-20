use crate::optimization_alternatives_v3::AlternativeCatalog;
use crate::optimization_emit_v3::{
    MechanicalEmitError, MechanicalModuleEmission, emit_mechanical_plan_v3,
};
use crate::optimization_evidence_v3::{PlannerFactHints, planner_facts_from_profile_evidence_v3};
use crate::optimization_plan::FunctionProfileEvidence;
use crate::optimization_plan_v3::{
    FunctionPlanIdentity, ModuleOptimizationPlanV3, ModulePlanIdentity, PlanDiagnostic, RegionId,
};
use crate::optimization_planner_v3::{
    ExtractedRegionPlanRequest, FunctionPlanRequest, ModulePlanRequest, plan_module_optimization_v3,
};
use crate::optimization_region_v3::{
    RegionExtractionAttempt, RegionExtractionError, extract_function_regions_v3,
};
use soac_core::block_py::{BlockLabel, BlockPyFunction};
use soac_lowering::passes::CodegenModuleShape;
use std::collections::HashMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactIntBranchV3Artifacts {
    pub plan: ModuleOptimizationPlanV3,
    pub emission: MechanicalModuleEmission,
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

fn extraction_diagnostic(block: BlockLabel, error: RegionExtractionError) -> PlanDiagnostic {
    PlanDiagnostic {
        source: None,
        message: format!("v3 extraction declined block {block}: {error}"),
    }
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
