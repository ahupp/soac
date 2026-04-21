use crate::alternatives_v3::AlternativeCatalog;
use crate::artifacts_v3::{ExactIntBranchV3Artifacts, write_optimization_artifacts_v3};
use crate::emit_v3::{MechanicalEmitError, emit_mechanical_plan_v3};
use crate::evidence_v3::{
    PlannerFactHints, planner_fact_hints_from_module_constants_v3,
    planner_facts_from_profile_evidence_v3,
};
use crate::plan::{
    CachedModuleOptimizationInput, FunctionProfileEvidence, ModuleOptimizationPlanReport,
    OptimizationPlanGenerationSummary, ProfileEvidenceStore, cached_module_paths_under_root,
};
use crate::plan_v3::{
    DirectCallArgPlan, DirectCallArgSource, FunctionPlanIdentity, IndexedFieldAccessKind,
    IndexedFieldOwnerType, ModulePlanIdentity, PlanDiagnostic, RegionId,
};
use crate::planner_v3::{
    DirectCallPlanRequest, ExtractedRegionPlanRequest, FunctionPlanRequest,
    IndexedFieldPlanRequest, ModulePlanRequest, plan_module_optimization_v3,
};
use crate::region_v3::{
    RegionExtractionAttempt, RegionExtractionError, extract_function_regions_v3,
};
use anyhow::{Context, Result, anyhow, bail};
use soac_core::block_py::{
    BlockLabel, BlockPyFunction, BlockPyModule, Call, CallArgPositional, ChildVisitable,
    FunctionExecutionMode, HasSemanticInstrId, InstrId, LocalFunctionId, NameLocation, ParamKind,
    RuntimeModuleId, SerializedFunctionId, SerializedModuleId, Visit,
};
use soac_driver::codegen_cache::{
    CachedCodegenModule, CachedCodegenModuleMetadata, load_codegen_module_cache,
    module_optimization_plan_v3_path,
};
use soac_lowering::block_py::literal::Literal;
use soac_lowering::passes::{CodegenModuleShape, InstrCodegen, InstrResolved};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

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
        let (direct_calls, direct_call_diagnostics) =
            direct_call_requests_from_same_module_evidence_v3(
                SerializedModuleId::new(0),
                function.function_id.runtime_module_id(),
                function,
                lowered_module,
                &evidence,
            );
        diagnostics.extend(direct_call_diagnostics);
        let indexed_fields = indexed_field_requests_from_type_key_evidence_v3(
            function,
            lowered_module,
            evidence_store,
        );
        functions.push(FunctionPlanRequest {
            function: function_plan_identity_v3(function),
            regions: region_requests,
            direct_calls,
            indexed_fields,
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
                direct_calls: Vec::new(),
                indexed_fields: Vec::new(),
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

fn indexed_field_requests_from_type_key_evidence_v3(
    function: &BlockPyFunction<CodegenModuleShape>,
    lowered_module: &BlockPyModule<CodegenModuleShape>,
    evidence_store: &ProfileEvidenceStore,
) -> Vec<IndexedFieldPlanRequest> {
    struct Collector<'a> {
        lowered_module: &'a BlockPyModule<CodegenModuleShape>,
        evidence_store: &'a ProfileEvidenceStore,
        requests: Vec<IndexedFieldPlanRequest>,
    }

    impl Collector<'_> {
        fn collect_attr(
            &mut self,
            source: InstrId,
            access: IndexedFieldAccessKind,
            attr_expr: &InstrCodegen,
        ) {
            let Some(attr_name) = codegen_constant_string_value_v3(self.lowered_module, attr_expr)
            else {
                return;
            };
            let Some(specializations) = self
                .evidence_store
                .field_index_specializations_for_attr(attr_name)
            else {
                return;
            };
            for specialization in specializations {
                self.requests.push(IndexedFieldPlanRequest {
                    source,
                    access,
                    owner_type: IndexedFieldOwnerType {
                        module_name: specialization.owner_type.module_name.clone(),
                        qualname: specialization.owner_type.qualname.clone(),
                    },
                    attr_name: specialization.attr_name.clone(),
                    expected_index: specialization.expected_index,
                    reason: "profiled type_keys selected this indexed-field layout for a constant attribute access".to_string(),
                });
            }
        }
    }

    impl Visit<InstrCodegen> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            match expr {
                InstrCodegen::GetAttr(op) => {
                    self.collect_attr(
                        op.semantic_instr_id(),
                        IndexedFieldAccessKind::Load,
                        op.attr.as_ref(),
                    );
                }
                InstrCodegen::SetAttr(op) => {
                    self.collect_attr(
                        op.semantic_instr_id(),
                        IndexedFieldAccessKind::Store,
                        op.attr.as_ref(),
                    );
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        lowered_module,
        evidence_store,
        requests: Vec::new(),
    };
    collector.visit_fn(function);
    collector.requests
}

fn codegen_constant_string_value_v3<'a>(
    module: &'a BlockPyModule<CodegenModuleShape>,
    expr: &InstrCodegen,
) -> Option<&'a str> {
    let InstrCodegen::Load(load) = expr else {
        return None;
    };
    let NameLocation::Constant(constant_index) = load.name.location else {
        return None;
    };
    module_constant_string_value_v3(module, constant_index)
}

fn module_constant_string_value_v3(
    module: &BlockPyModule<CodegenModuleShape>,
    constant_index: u32,
) -> Option<&str> {
    let InstrResolved::Literal(literal) = module.module_constants.get(constant_index as usize)?
    else {
        return None;
    };
    let Literal::StringLiteral(literal) = literal.as_literal() else {
        return None;
    };
    Some(literal.value.as_str())
}

fn direct_call_requests_from_same_module_evidence_v3(
    serialized_module_id: SerializedModuleId,
    runtime_module_id: RuntimeModuleId,
    function: &BlockPyFunction<CodegenModuleShape>,
    lowered_module: &BlockPyModule<CodegenModuleShape>,
    evidence: &FunctionProfileEvidence,
) -> (Vec<DirectCallPlanRequest>, Vec<PlanDiagnostic>) {
    let mut requests = Vec::new();
    let mut diagnostics = Vec::new();
    let mut entries = evidence
        .call_target_specializations
        .iter()
        .collect::<Vec<_>>();
    entries.sort_by_key(|(source, _)| **source);
    for (source, targets) in entries {
        let mut targets = targets.clone();
        targets.sort();
        targets.dedup();
        for target in targets {
            if target.runtime_module_id() != runtime_module_id {
                continue;
            }
            let serialized_target =
                SerializedFunctionId::new(serialized_module_id, target.local_function_id());
            let Some(target_function) = lowered_module
                .callable_defs
                .iter()
                .find(|candidate| candidate.function_id == target)
            else {
                diagnostics.push(PlanDiagnostic {
                    source: Some(*source),
                    message: format!(
                        "v3 direct-call declined target {serialized_target}: target function is missing from lowered module"
                    ),
                });
                continue;
            };
            if target_function.execution_mode() != FunctionExecutionMode::Jit {
                diagnostics.push(PlanDiagnostic {
                    source: Some(*source),
                    message: format!(
                        "v3 direct-call declined target {serialized_target}: target function is not JIT lowered"
                    ),
                });
                continue;
            }
            if target_function.names.fn_name == "__init__" {
                diagnostics.push(PlanDiagnostic {
                    source: Some(*source),
                    message: format!(
                        "v3 direct-call declined target {serialized_target}: constructor targets require owner/type guards"
                    ),
                });
                continue;
            }
            let arg_plan = match direct_call_arg_plan_for_instr_id_v3(
                function,
                *source,
                target_function,
            ) {
                Some(Ok(arg_plan)) => arg_plan,
                Some(Err(reason)) => {
                    diagnostics.push(PlanDiagnostic {
                        source: Some(*source),
                        message: format!(
                            "v3 direct-call declined target {serialized_target}: {reason}"
                        ),
                    });
                    continue;
                }
                None => {
                    diagnostics.push(PlanDiagnostic {
                        source: Some(*source),
                        message: format!(
                            "v3 direct-call declined target {serialized_target}: source instruction is not a lowered call"
                        ),
                    });
                    continue;
                }
            };
            requests.push(DirectCallPlanRequest {
                source: *source,
                target: serialized_target,
                arg_plan,
                reason: "profiled call_hot_targets selected this same-module function with validated ordinary-call arguments".to_string(),
            });
        }
    }
    (requests, diagnostics)
}

fn direct_call_arg_plan_for_instr_id_v3(
    function: &BlockPyFunction<CodegenModuleShape>,
    source: InstrId,
    target_function: &BlockPyFunction<CodegenModuleShape>,
) -> Option<std::result::Result<DirectCallArgPlan, String>> {
    struct Finder<'a> {
        source: InstrId,
        target_function: &'a BlockPyFunction<CodegenModuleShape>,
        result: Option<std::result::Result<DirectCallArgPlan, String>>,
    }

    impl Visit<InstrCodegen> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if self.result.is_some() {
                return;
            }
            if let InstrCodegen::Call(call) = expr
                && call.try_semantic_instr_id() == Some(self.source)
            {
                self.result = Some(direct_call_arg_plan_from_call_v3(
                    call,
                    self.target_function,
                ));
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        source,
        target_function,
        result: None,
    };
    finder.visit_fn(function);
    finder.result
}

fn direct_call_arg_plan_from_call_v3(
    call: &Call<InstrCodegen>,
    target_function: &BlockPyFunction<CodegenModuleShape>,
) -> std::result::Result<DirectCallArgPlan, String> {
    if call
        .args
        .iter()
        .any(|arg| matches!(arg, CallArgPositional::Starred(_)))
    {
        return Err("starred arguments are not supported".to_string());
    }
    if !call.keywords.is_empty() {
        return Err("keyword arguments are not supported".to_string());
    }

    for param in target_function.params.iter() {
        if matches!(param.kind, ParamKind::VarArg | ParamKind::KwArg) {
            return Err(format!(
                "target parameter kind {:?} is not supported",
                param.kind
            ));
        }
    }

    let provided_positional_arg_count = call
        .args
        .iter()
        .filter(|arg| matches!(arg, CallArgPositional::Positional(_)))
        .count();
    let accepted_positional_arg_count = target_function
        .params
        .iter()
        .filter(|param| matches!(param.kind, ParamKind::PosOnly | ParamKind::Any))
        .count();
    if provided_positional_arg_count > accepted_positional_arg_count {
        return Err(format!(
            "too many positional arguments: provided {provided_positional_arg_count}, accepted {accepted_positional_arg_count}"
        ));
    }

    let mut sources = Vec::with_capacity(target_function.params.len());
    let mut next_provided_arg = 0usize;
    for param in target_function.params.iter() {
        match param.kind {
            ParamKind::PosOnly | ParamKind::Any => {
                if next_provided_arg < provided_positional_arg_count {
                    sources.push(DirectCallArgSource::Provided(
                        next_provided_arg
                            .try_into()
                            .map_err(|_| "too many positional arguments for v3 arg plan")?,
                    ));
                    next_provided_arg += 1;
                } else if param.has_default {
                    sources.push(DirectCallArgSource::DefaultSentinel);
                } else {
                    return Err(format!("missing required argument {}", param.name));
                }
            }
            ParamKind::KwOnly => {
                if param.has_default {
                    sources.push(DirectCallArgSource::DefaultSentinel);
                } else {
                    return Err(format!(
                        "missing required keyword-only argument {}",
                        param.name
                    ));
                }
            }
            ParamKind::VarArg | ParamKind::KwArg => unreachable!(
                "unsupported variadic params should be rejected before planning direct-call args"
            ),
        }
    }
    debug_assert_eq!(next_provided_arg, provided_positional_arg_count);
    Ok(DirectCallArgPlan { sources })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator_specialization::{ExactTypeTag, pack_binary_shape};
    use crate::plan_v3::{RegionId, validate_module_plan_v3};
    use crate::region_v3::{ExtractedValueId, extract_block_region_v3};
    use soac_core::block_py::{
        BinOp, BinOpKind, Block, BlockLabel, BlockParam, BlockPyName, BlockTerm, FunctionName,
        GetAttr, InstrId, Load, LocalFunctionId, LocalLocation, Meta, ModuleNameGen, NameLocation,
        ParamSpec, ResolvedName, RuntimeFunctionId, SerializedFunctionId, SerializedModuleId,
        SetAttr, TermIf, WithMeta,
    };
    use soac_core::profile::{
        CounterDumpRecord, CounterDumpTypeKey, CounterDumpTypeKeyLayout, CounterDumpTypeTableEntry,
    };
    use soac_lowering::block_py::literal::{LiteralValue, StringLiteral};
    use soac_lowering::passes::{InstrCodegen, InstrResolved};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    fn constant_name(index: u32) -> InstrCodegen {
        InstrCodegen::Load(Load::new(ResolvedName {
            id: BlockPyName::new(format!("<const {index}>")),
            location: NameLocation::Constant(index),
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

    fn function_with_blocks(
        blocks: Vec<Block<InstrCodegen>>,
    ) -> BlockPyFunction<CodegenModuleShape> {
        let name_gen = ModuleNameGen::new(0).next_function_name_gen();
        BlockPyFunction {
            function_id: name_gen.function_id(),
            name_gen,
            names: FunctionName::new("f", "f", "f", "f"),
            kind: soac_core::block_py::FunctionKind::Function,
            execution_mode: Default::default(),
            params: ParamSpec::default(),
            blocks,
            doc: None,
            storage_layout: None,
            scope: Default::default(),
        }
    }

    fn unique_counter_path_v3() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac_opt-v3-pipeline-test-{}-{nanos}.bin",
            std::process::id()
        ))
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
    fn direct_call_evidence_without_module_context_is_not_planned() {
        let source = instr_id(9);
        let mut evidence = FunctionProfileEvidence::default();
        evidence.call_target_specializations.insert(
            source,
            vec![
                RuntimeFunctionId::from_raw_parts(0, 2),
                RuntimeFunctionId::from_raw_parts(99, 3),
            ],
        );

        let artifacts = plan_and_emit_extracted_exact_int_branches_v3(
            &AlternativeCatalog::default_v3(),
            module_identity(),
            function_identity(),
            Vec::new(),
            &evidence,
            &HashMap::new(),
        )
        .unwrap();

        let direct_calls = &artifacts.plan.functions[0].direct_calls;
        assert!(
            direct_calls.is_empty(),
            "v3 direct-call planning requires the lowered call site and target signature"
        );
    }

    #[test]
    fn indexed_field_requests_are_derived_from_raw_type_key_evidence() {
        let attr_name = constant_name(0);
        let get_source = InstrId::new(label(0), 5);
        let set_source = InstrId::new(label(0), 8);
        let block = Block::new(
            label(0),
            vec![
                InstrCodegen::GetAttr(GetAttr::new(local("record", 0), attr_name.clone()))
                    .with_meta(Meta {
                        instr_id: Some(get_source),
                        ..Meta::synthetic()
                    }),
                InstrCodegen::SetAttr(SetAttr::new(
                    local("record", 0),
                    attr_name,
                    local("value", 1),
                ))
                .with_meta(Meta {
                    instr_id: Some(set_source),
                    ..Meta::synthetic()
                }),
            ],
            BlockTerm::jump_term(label(1)),
            Vec::<BlockParam>::new(),
            None,
        );
        let function = function_with_blocks(vec![block]);
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function],
            module_constants: vec![InstrResolved::Literal(LiteralValue::new(StringLiteral {
                value: "value".to_string(),
            }))],
            counter_defs: Vec::new(),
        };
        let record = CounterDumpRecord {
            source_hash: 0x1234,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows: Vec::new(),
            module_keys: Vec::new(),
            type_keys: vec![CounterDumpTypeKeyLayout {
                owner_type_id: 44,
                key: "value".to_string(),
                index: 2,
            }],
            type_table: vec![CounterDumpTypeTableEntry {
                type_id: 44,
                key: CounterDumpTypeKey {
                    module_name: "pkg.model".to_string(),
                    qualname: "Record".to_string(),
                },
            }],
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);

        let requests = indexed_field_requests_from_type_key_evidence_v3(
            &module.callable_defs[0],
            &module,
            &evidence_store,
        );

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].source, get_source);
        assert_eq!(requests[0].access, IndexedFieldAccessKind::Load);
        assert_eq!(requests[0].owner_type.module_name, "pkg.model");
        assert_eq!(requests[0].owner_type.qualname, "Record");
        assert_eq!(requests[0].attr_name, "value");
        assert_eq!(requests[0].expected_index, 2);
        assert_eq!(requests[1].source, set_source);
        assert_eq!(requests[1].access, IndexedFieldAccessKind::Store);
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
