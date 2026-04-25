use crate::operator_specialization::{ExactTypeTag, unpack_binary_shape};
use crate::plan::FunctionProfileEvidence;
use crate::planner_v3::PlannerFacts;
use crate::region_v3::{ExtractedRegion, ExtractedValueId, ExtractedValueKind};
use soac_core::block_py::literal::{Literal, NumberLiteralValue};
use soac_core::block_py::{ConstantExpr, NameLocation};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlannerFactHints {
    i64_constants: HashMap<ExtractedValueId, i64>,
}

impl PlannerFactHints {
    pub fn set_i64_constant(&mut self, value: ExtractedValueId, constant: i64) {
        self.i64_constants.insert(value, constant);
    }
}

pub fn planner_facts_from_profile_evidence_v3(
    region: &ExtractedRegion,
    evidence: &FunctionProfileEvidence,
    hints: &PlannerFactHints,
) -> PlannerFacts {
    let mut facts = PlannerFacts::default();
    for (value, constant) in &hints.i64_constants {
        facts.set_i64_constant(*value, *constant);
    }

    for value in &region.values {
        let ExtractedValueKind::Binary { left, right, .. } = value.kind else {
            continue;
        };
        let Some(source) = value.source else {
            continue;
        };
        let Some(shapes) = evidence.operator_specializations.get(&source) else {
            continue;
        };
        if has_exact_int_binary_shape(shapes) {
            facts.mark_exact_compact_int(left);
            facts.mark_exact_compact_int(right);
        }
    }

    facts
}

pub fn planner_fact_hints_from_module_constants_v3(
    region: &ExtractedRegion,
    module_constants: &[ConstantExpr],
) -> PlannerFactHints {
    let mut hints = PlannerFactHints::default();
    for value in &region.values {
        let ExtractedValueKind::LoadName { name } = &value.kind else {
            continue;
        };
        let NameLocation::Constant(index) = name.location else {
            continue;
        };
        let Some(constant) = module_i64_constant(module_constants, index) else {
            continue;
        };
        hints.set_i64_constant(value.id, constant);
    }
    hints
}

fn module_i64_constant(module_constants: &[ConstantExpr], index: u32) -> Option<i64> {
    let ConstantExpr::Literal(literal) = module_constants.get(index as usize)? else {
        return None;
    };
    let Literal::NumberLiteral(number) = literal.as_literal() else {
        return None;
    };
    let NumberLiteralValue::Int(value) = &number.value else {
        return None;
    };
    value.as_i64()
}

fn has_exact_int_binary_shape(shapes: &[u64]) -> bool {
    shapes
        .iter()
        .any(|shape| unpack_binary_shape(*shape) == Some((ExactTypeTag::Int, ExactTypeTag::Int)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alternatives_v3::AlternativeCatalog;
    use crate::operator_specialization::{ExactTypeTag, pack_binary_shape};
    use crate::plan::FunctionProfileEvidence;
    use crate::planner_v3::{
        ExtractedRegionPlanRequest, FunctionPlanRequest, ModulePlanRequest,
        plan_module_optimization_v3,
    };
    use crate::region_v3::extract_block_region_v3;
    use soac_core::block_py::literal::{IntLiteral, LiteralValue, NumberLiteral};
    use soac_core::block_py::{
        BinOp, BinOpKind, Block, BlockLabel, BlockParam, BlockPyName, BlockTerm, InstrId, Load,
        LocalFunctionId, LocalLocation, Meta, NameLocation, ResolvedName, SerializedFunctionId,
        SerializedIdentityTables, SerializedModuleId, SerializedModuleIdentity, TermIf, WithMeta,
    };
    use soac_ir_typed::plan_v3::{
        FunctionPlanIdentity, ModulePlanIdentity, RegionId, validate_module_plan_v3,
    };

    fn label(index: usize) -> BlockLabel {
        BlockLabel::from_index(index)
    }

    fn instr_id(index: u32) -> InstrId {
        InstrId::new(index)
    }

    fn with_instr_id(
        instr: soac_ir_blockpy::InstrCodegen,
        index: u32,
    ) -> soac_ir_blockpy::InstrCodegen {
        instr.with_meta(Meta {
            instr_id: Some(instr_id(index)),
            ..Meta::synthetic()
        })
    }

    fn local(name: &str, slot: u32) -> soac_ir_blockpy::InstrCodegen {
        soac_ir_blockpy::InstrCodegen::Load(Load::new(ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Local(LocalLocation(slot)),
        }))
    }

    fn constant(index: u32) -> soac_ir_blockpy::InstrCodegen {
        soac_ir_blockpy::InstrCodegen::Load(Load::new(ResolvedName {
            id: BlockPyName::new("__dp_constant"),
            location: NameLocation::Constant(index),
        }))
    }

    fn binary(
        op: BinOpKind,
        left: soac_ir_blockpy::InstrCodegen,
        right: soac_ir_blockpy::InstrCodegen,
        id: u32,
    ) -> soac_ir_blockpy::InstrCodegen {
        with_instr_id(
            soac_ir_blockpy::InstrCodegen::BinOp(BinOp::new(op, left, right)),
            id,
        )
    }

    fn compact_region() -> ExtractedRegion {
        let add = binary(
            BinOpKind::Add,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let test = binary(BinOpKind::Gt, add, with_instr_id(local("zero", 2), 3), 4);
        let block = Block::new(
            label(0),
            Vec::new(),
            BlockTerm::IfTerm(TermIf {
                test,
                then_label: label(1),
                else_label: label(2),
            }),
            Vec::<BlockParam>::new(),
            None,
        );
        extract_block_region_v3(&block, RegionId(0)).unwrap()
    }

    fn compact_region_with_constant_zero() -> ExtractedRegion {
        let add = binary(
            BinOpKind::Add,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let test = binary(BinOpKind::Gt, add, with_instr_id(constant(0), 3), 4);
        let block = Block::new(
            label(0),
            Vec::new(),
            BlockTerm::IfTerm(TermIf {
                test,
                then_label: label(1),
                else_label: label(2),
            }),
            Vec::<BlockParam>::new(),
            None,
        );
        extract_block_region_v3(&block, RegionId(0)).unwrap()
    }

    fn int_constant(value: i64) -> ConstantExpr {
        ConstantExpr::Literal(LiteralValue::new(Literal::NumberLiteral(NumberLiteral {
            value: NumberLiteralValue::Int(IntLiteral::from_i64(value)),
        })))
    }

    fn evidence_with_add_shape(shape: u64) -> FunctionProfileEvidence {
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .operator_specializations
            .insert(instr_id(2), vec![shape]);
        evidence
    }

    fn hints() -> PlannerFactHints {
        let mut hints = PlannerFactHints::default();
        hints.set_i64_constant(ExtractedValueId(3), 0);
        hints
    }

    fn plan_with(
        region: ExtractedRegion,
        facts: PlannerFacts,
    ) -> soac_ir_typed::plan_v3::ModuleOptimizationPlanV3 {
        plan_module_optimization_v3(
            &AlternativeCatalog::default_v3(),
            ModulePlanRequest {
                module: ModulePlanIdentity {
                    module_name: "pkg.mod".to_string(),
                    source_hash: 0x88,
                    cache_identity: "test-cache".to_string(),
                },
                identity_tables: SerializedIdentityTables {
                    modules: vec![SerializedModuleIdentity {
                        module_name: "pkg.mod".to_string(),
                        source_hash: 0x88,
                        cache_identity: Some("test-cache".to_string()),
                    }],
                    debug_names: Vec::new(),
                },
                functions: vec![FunctionPlanRequest {
                    function: FunctionPlanIdentity {
                        function: SerializedFunctionId::new(
                            SerializedModuleId::new(0),
                            LocalFunctionId::new(1),
                        ),
                        debug_name: Some("f".to_string()),
                    },
                    regions: vec![ExtractedRegionPlanRequest { region, facts }],
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                }],
            },
        )
    }

    #[test]
    fn exact_int_operator_evidence_enables_compact_int_plan() {
        let region = compact_region();
        let evidence =
            evidence_with_add_shape(pack_binary_shape(ExactTypeTag::Int, ExactTypeTag::Int));
        let facts = planner_facts_from_profile_evidence_v3(&region, &evidence, &hints());
        let plan = plan_with(region, facts);

        validate_module_plan_v3(&plan).unwrap();
        assert_eq!(plan.functions[0].regions.len(), 2);
        assert!(plan.functions[0].diagnostics.is_empty());
    }

    #[test]
    fn module_constant_hint_enables_compact_int_branch_plan() {
        let region = compact_region_with_constant_zero();
        let evidence =
            evidence_with_add_shape(pack_binary_shape(ExactTypeTag::Int, ExactTypeTag::Int));
        let hints = planner_fact_hints_from_module_constants_v3(&region, &[int_constant(0)]);
        let facts = planner_facts_from_profile_evidence_v3(&region, &evidence, &hints);
        let plan = plan_with(region, facts);

        validate_module_plan_v3(&plan).unwrap();
        assert_eq!(plan.functions[0].regions.len(), 2);
        assert!(plan.functions[0].diagnostics.is_empty());
    }

    #[test]
    fn non_exact_int_operator_evidence_does_not_enable_plan() {
        let region = compact_region();
        let evidence = evidence_with_add_shape(0);
        let facts = planner_facts_from_profile_evidence_v3(&region, &evidence, &hints());
        let plan = plan_with(region, facts);

        assert!(plan.functions[0].regions.is_empty());
        assert_eq!(plan.functions[0].diagnostics.len(), 1);
    }

    #[test]
    fn comparison_evidence_alone_does_not_prove_add_operands() {
        let region = compact_region();
        let mut evidence = FunctionProfileEvidence::default();
        evidence.operator_specializations.insert(
            instr_id(4),
            vec![pack_binary_shape(ExactTypeTag::Int, ExactTypeTag::Int)],
        );
        let facts = planner_facts_from_profile_evidence_v3(&region, &evidence, &hints());
        let plan = plan_with(region, facts);

        assert!(plan.functions[0].regions.is_empty());
        assert_eq!(plan.functions[0].diagnostics.len(), 1);
    }
}
