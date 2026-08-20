use crate::alternatives_v3::AlternativeCatalog;
use crate::artifacts_v3::ExactIntBranchV3Artifacts;
use crate::evidence_v3::{
    PlannerFactHints, planner_fact_hints_from_module_constants_v3,
    planner_facts_from_profile_evidence_v3,
};
use crate::operator_specialization::{ExactTypeTag, pack_binary_shape};
use crate::passes::{
    BlockPyModuleShape, InlineUnsupportedReason, InlineValueBindings, InstrBlockPy,
    build_cross_module_direct_call_inline_fragment_to_target,
    build_direct_call_inline_fragment_to_target, try_allocate_codegen_stack_temp,
};
use crate::plan::{FunctionProfileEvidence, ProfileEvidenceStore};
use crate::planner_v3::{
    CallBodyPlanRequest, DirectCallPlanRequest, ExactListItemPlanRequest,
    ExtractedRegionPlanRequest, FunctionPlanRequest, IndexedFieldPlanRequest,
    IndexedGlobalPlanRequest, ModulePlanRequest, plan_module_optimization_v3,
};
use crate::region_v3::{
    RegionExtractionAttempt, RegionExtractionError, extract_function_regions_v3,
};
use anyhow::Result;
use soac_core::block_py::literal::Literal;
use soac_core::block_py::{
    BinOpKind, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, Call, CallArgPositional,
    CallableScopeKind, ChildVisitable, ConstantExpr, FunctionExecutionMode, FunctionKind,
    HasSemanticInstrId, InstrId, LocalFunctionId, LocalLocation, ModuleContentId, NameLike,
    NameLocation, ParamKind, PersistentFunctionId, ResolvedName, RuntimeFunctionId, RuntimeName,
    SerializedFunctionDebugName, SerializedFunctionId, SerializedIdentityTables,
    SerializedModuleId, SerializedModuleIdentity, Tuple, Visit,
};
use soac_ir_blockpy::is_constructor_entry_function;
use soac_ir_typed::emit_v3::{MechanicalEmitError, emit_mechanical_plan_v3};
use soac_ir_typed::plan_v3::{
    DirectCallArgPlan, DirectCallArgSource, DirectCallCallee, DirectCallSpecializationPlan,
    EXACT_LIST_EXACT_INT_ITEM_SHAPE_TAG, EXACT_TUPLE_EXACT_INT_ITEM_SHAPE_TAG,
    ExactFloatExpressionOperationPlan, ExactFloatExpressionSpecializationPlan,
    ExactListItemAccessKind, ExactListItemShape, ExactListItemSpecializationPlan,
    FunctionOptimizationPlanV3, FunctionPlanIdentity, IndexedFieldAccessKind,
    IndexedFieldOwnerType, IndexedFieldSpecializationPlan, IndexedGlobalAccessKind,
    IndexedGlobalSpecializationPlan, LateBoundOwnerFieldSpecializationPlan,
    LateBoundOwnerFieldStorage, ModuleOptimizationPlanV3, ModulePlanIdentity, PlanDiagnostic,
    RegionId, RegionInputSource, RegionPlan, Rep,
};
use std::collections::{HashMap, HashSet};
use std::fmt;

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

#[derive(Clone, Debug, Default)]
pub struct DirectCallTargetIndex {
    functions: HashMap<PersistentFunctionId, DirectCallTargetEntry>,
}

#[derive(Clone, Debug)]
struct DirectCallTargetEntry {
    module: ModuleContentId,
    function: BlockPyFunction<BlockPyModuleShape>,
    module_constants: Vec<ConstantExpr>,
}

#[derive(Clone, Debug)]
pub struct ModuleOptimizationInput<'a> {
    pub identity: ModulePlanIdentity,
    pub module: &'a BlockPyModule<BlockPyModuleShape>,
    pub strict: bool,
}

impl<'a> ModuleOptimizationInput<'a> {
    pub fn new(
        identity: ModulePlanIdentity,
        module: &'a BlockPyModule<BlockPyModuleShape>,
        strict: bool,
    ) -> Self {
        Self {
            identity,
            module,
            strict,
        }
    }
}

#[derive(Debug)]
pub struct PlannedModuleV3 {
    pub identity: ModulePlanIdentity,
    pub artifacts: ExactIntBranchV3Artifacts,
}

#[derive(Debug, Default)]
pub struct OptimizeModulesV3Output {
    pub modules: Vec<PlannedModuleV3>,
    pub skipped: usize,
}

impl DirectCallTargetIndex {
    fn from_current_module(
        identity: &ModulePlanIdentity,
        module: &BlockPyModule<BlockPyModuleShape>,
    ) -> Self {
        let mut index = Self::default();
        index.insert_module(identity, module);
        index
    }

    fn from_modules(module_inputs: &[ModuleOptimizationInput<'_>]) -> DirectCallTargetIndex {
        let mut index = DirectCallTargetIndex::default();
        for module_input in module_inputs {
            index.insert_module(&module_input.identity, module_input.module);
        }
        index
    }

    fn insert_module(
        &mut self,
        identity: &ModulePlanIdentity,
        module: &BlockPyModule<BlockPyModuleShape>,
    ) {
        let content_id = ModuleContentId::new(identity.module_name.clone(), identity.source_hash);
        for function in &module.callable_defs {
            let persistent_id = PersistentFunctionId::new(
                content_id.clone(),
                function.function_id.local_function_id(),
            );
            self.functions.insert(
                persistent_id.clone(),
                DirectCallTargetEntry {
                    module: content_id.clone(),
                    function: function.clone(),
                    module_constants: module.module_constants.clone(),
                },
            );
        }
    }

    fn entry(&self, function_id: &PersistentFunctionId) -> Option<&DirectCallTargetEntry> {
        self.functions.get(function_id)
    }
}

struct OptimizationPlanV3IdentityBuilder {
    tables: SerializedIdentityTables,
    modules_by_content: HashMap<ModuleContentId, SerializedModuleId>,
}

impl OptimizationPlanV3IdentityBuilder {
    fn new(identity: &ModulePlanIdentity) -> Self {
        let mut builder = Self {
            tables: SerializedIdentityTables::default(),
            modules_by_content: HashMap::new(),
        };
        builder.intern_module(
            ModuleContentId::new(identity.module_name.clone(), identity.source_hash),
            Some(identity.cache_identity.clone()),
        );
        builder
    }

    fn function_id_for_persistent(
        &mut self,
        function: PersistentFunctionId,
    ) -> SerializedFunctionId {
        let module_id = self.intern_module(function.module, None);
        SerializedFunctionId::new(module_id, function.local)
    }

    fn add_debug_name(&mut self, function: SerializedFunctionId, qualname: impl Into<String>) {
        self.tables.debug_names.push(SerializedFunctionDebugName {
            function,
            qualname: qualname.into(),
        });
    }

    fn finish(self) -> SerializedIdentityTables {
        self.tables
    }

    fn intern_module(
        &mut self,
        module: ModuleContentId,
        cache_identity: Option<String>,
    ) -> SerializedModuleId {
        if let Some(module_id) = self.modules_by_content.get(&module) {
            if let Some(cache_identity) = cache_identity
                && let Some(identity) = self.tables.modules.get_mut(module_id.as_u32() as usize)
                && identity.cache_identity.is_none()
            {
                identity.cache_identity = Some(cache_identity);
            }
            return *module_id;
        }
        let module_id = SerializedModuleId::new(self.tables.modules.len() as u32);
        self.tables.modules.push(SerializedModuleIdentity {
            module_name: module.module_name.clone(),
            source_hash: module.source_hash,
            cache_identity,
        });
        self.modules_by_content.insert(module, module_id);
        module_id
    }
}

fn identity_tables_for_single_module(module: &ModulePlanIdentity) -> SerializedIdentityTables {
    SerializedIdentityTables {
        modules: vec![SerializedModuleIdentity {
            module_name: module.module_name.clone(),
            source_hash: module.source_hash,
            cache_identity: Some(module.cache_identity.clone()),
        }],
        debug_names: Vec::new(),
    }
}

pub fn plan_and_emit_function_exact_int_branches_v3(
    catalog: &AlternativeCatalog,
    module: ModulePlanIdentity,
    function: FunctionPlanIdentity,
    lowered_function: &BlockPyFunction<BlockPyModuleShape>,
    evidence: &FunctionProfileEvidence,
    hints_by_region: &HashMap<RegionId, PlannerFactHints>,
) -> Result<ExactIntBranchV3Artifacts, ExactIntBranchV3Error> {
    let attempts = extract_function_regions_v3(lowered_function);
    let mut artifacts = plan_and_emit_extracted_exact_int_branches_v3(
        catalog,
        module,
        function,
        attempts,
        evidence,
        hints_by_region,
    )?;
    attach_exact_float_expression_plans(&mut artifacts, lowered_function, evidence)?;
    Ok(artifacts)
}

pub fn plan_and_emit_function_exact_int_branches_v3_with_module_constants(
    catalog: &AlternativeCatalog,
    module: ModulePlanIdentity,
    function: FunctionPlanIdentity,
    lowered_function: &BlockPyFunction<BlockPyModuleShape>,
    evidence: &FunctionProfileEvidence,
    module_constants: &[ConstantExpr],
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
    let mut artifacts = plan_and_emit_extracted_exact_int_branches_v3(
        catalog,
        module,
        function,
        attempts,
        evidence,
        &hints_by_region,
    )?;
    attach_exact_float_expression_plans(&mut artifacts, lowered_function, evidence)?;
    Ok(artifacts)
}

fn attach_exact_float_expression_plans(
    artifacts: &mut ExactIntBranchV3Artifacts,
    lowered_function: &BlockPyFunction<BlockPyModuleShape>,
    evidence: &FunctionProfileEvidence,
) -> Result<(), ExactIntBranchV3Error> {
    if let Some(planned_function) = artifacts.plan.functions.first_mut() {
        planned_function.exact_float_expressions =
            exact_float_expression_plans_from_profile_evidence_v3(lowered_function, evidence);
    }
    artifacts.emission =
        emit_mechanical_plan_v3(&artifacts.plan).map_err(ExactIntBranchV3Error::Emit)?;
    Ok(())
}

pub fn plan_and_emit_module_v3_from_raw_evidence(
    catalog: &AlternativeCatalog,
    module_identity: ModulePlanIdentity,
    lowered_module: &BlockPyModule<BlockPyModuleShape>,
    evidence_store: &ProfileEvidenceStore,
) -> Result<ExactIntBranchV3Artifacts, ExactIntBranchV3Error> {
    let target_index = DirectCallTargetIndex::from_current_module(&module_identity, lowered_module);
    plan_and_emit_module_v3_from_raw_evidence_with_target_index(
        catalog,
        &module_identity,
        lowered_module,
        evidence_store,
        &target_index,
    )
}

fn plan_and_emit_module_v3_from_raw_evidence_with_target_index(
    catalog: &AlternativeCatalog,
    module_identity: &ModulePlanIdentity,
    lowered_module: &BlockPyModule<BlockPyModuleShape>,
    evidence_store: &ProfileEvidenceStore,
    target_index: &DirectCallTargetIndex,
) -> Result<ExactIntBranchV3Artifacts, ExactIntBranchV3Error> {
    let module = module_identity.clone();
    let mut identity_builder = OptimizationPlanV3IdentityBuilder::new(module_identity);
    let mut functions = Vec::new();
    let mut diagnostics_by_function = Vec::new();
    let mut exact_float_expressions_by_function = Vec::new();
    let owner_field_sites =
        late_bound_owner_field_site_catalog(lowered_module, module_identity.module_name.as_str());
    let split_owner_names = owner_field_sites
        .iter()
        .filter(|(_, site)| matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. }))
        .map(|(_, site)| site.owner_type.qualname.clone())
        .collect::<HashSet<_>>();
    let inherited_split_owners =
        literal_same_module_class_ancestors(lowered_module, &split_owner_names);
    let mut late_bound_owner_fields_by_function = Vec::new();
    let mut hot_field_accesses_by_function = Vec::new();
    for function in &lowered_module.callable_defs {
        let attempts = if function_uses_generator_resume_state(function) {
            Vec::new()
        } else {
            extract_function_regions_v3(function)
        };
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
            module_identity.module_name.as_str(),
            module_identity.source_hash,
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
        let (direct_calls, direct_call_diagnostics) = direct_call_requests_from_evidence_v3(
            lowered_module,
            module_identity,
            function,
            evidence_store,
            target_index,
            &mut identity_builder,
        );
        diagnostics.extend(direct_call_diagnostics);
        let (exact_list_items, exact_list_item_diagnostics) =
            exact_list_item_requests_from_profile_evidence_v3(function, &evidence);
        diagnostics.extend(exact_list_item_diagnostics);
        let indexed_fields = indexed_field_requests_from_type_key_evidence_v3(
            function,
            lowered_module,
            evidence_store,
        );
        let indexed_globals = indexed_global_requests_from_module_key_evidence_v3(
            module_identity.module_name.as_str(),
            function,
            evidence_store,
        );
        exact_float_expressions_by_function.push(
            exact_float_expression_plans_from_profile_evidence_v3(function, &evidence),
        );
        let lexical_owner = function
            .names
            .qualname
            .rsplit_once('.')
            .map(|(owner, _)| owner);
        let mut selected_owner_fields = Vec::new();
        for (_, site) in owner_field_sites.iter().filter(|(function_id, site)| {
            *function_id == function.function_id
                && lexical_owner == Some(site.owner_type.qualname.as_str())
                && evidence
                    .hot_field_accesses
                    .get(&site.source)
                    .copied()
                    .unwrap_or_default()
                    >= 8
        }) {
            if !matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. }) {
                selected_owner_fields.push(site.clone());
                continue;
            }

            let Some(layouts) =
                evidence_store.field_index_specializations_for_attr(site.attr_name.as_str())
            else {
                continue;
            };
            let mut owners = owner_field_sites
                .iter()
                .filter(|(_, anchor)| {
                    anchor.attr_name == site.attr_name
                        && matches!(anchor.storage, LateBoundOwnerFieldStorage::SplitDict { .. })
                        && (anchor.owner_type == site.owner_type
                            || inherited_split_owners
                                .get(anchor.owner_type.qualname.as_str())
                                .is_some_and(|ancestors| {
                                    ancestors.contains(site.owner_type.qualname.as_str())
                                }))
                })
                .collect::<Vec<_>>();
            owners.sort_by_key(|(_, anchor)| {
                (
                    anchor.owner_type == site.owner_type,
                    anchor.owner_type.qualname.as_str(),
                    anchor.cell_index,
                )
            });
            owners.dedup_by(|left, right| left.1.owner_type == right.1.owner_type);
            owners.retain(|(_, anchor)| {
                layouts.iter().any(|layout| {
                    layout.owner_type.module_name == anchor.owner_type.module_name
                        && layout.owner_type.qualname == anchor.owner_type.qualname
                })
            });
            if owners.len() > 8 {
                let lexical = owners
                    .iter()
                    .position(|(_, anchor)| anchor.owner_type == site.owner_type)
                    .map(|index| owners.remove(index));
                owners.truncate(if lexical.is_some() { 7 } else { 8 });
                if let Some(lexical) = lexical {
                    owners.push(lexical);
                }
            }

            for (_, anchor) in owners {
                let Some(layout) = layouts.iter().find(|layout| {
                    layout.owner_type.module_name == anchor.owner_type.module_name
                        && layout.owner_type.qualname == anchor.owner_type.qualname
                }) else {
                    continue;
                };
                let mut selected = site.clone();
                selected.owner_type = anchor.owner_type.clone();
                selected.storage = LateBoundOwnerFieldStorage::SplitDict {
                    expected_index: layout.expected_index,
                };
                selected.cell_index = anchor.cell_index;
                if selected.owner_type != site.owner_type {
                    selected.reason =
                        "profiled inherited receiver uses an exact concrete split-field owner"
                            .to_string();
                }
                selected_owner_fields.push(selected);
            }
        }
        late_bound_owner_fields_by_function.push(selected_owner_fields);
        hot_field_accesses_by_function.push(evidence.hot_field_accesses.clone());
        functions.push(FunctionPlanRequest {
            function: function_plan_identity_v3(function),
            regions: region_requests,
            direct_calls,
            exact_list_items,
            opaque_fused_iterations: Vec::new(),
            indexed_fields,
            indexed_globals,
        });
        diagnostics_by_function.push(diagnostics);
    }

    let identity_tables = identity_builder.finish();
    let mut plan = plan_module_optimization_v3(
        catalog,
        ModulePlanRequest {
            module,
            identity_tables,
            functions,
        },
    );
    for (
        (((function, diagnostics), exact_float_expressions), late_bound_owner_fields),
        hot_fields,
    ) in plan
        .functions
        .iter_mut()
        .zip(diagnostics_by_function)
        .zip(exact_float_expressions_by_function)
        .zip(late_bound_owner_fields_by_function)
        .zip(hot_field_accesses_by_function)
    {
        function.diagnostics.extend(diagnostics);
        function.exact_float_expressions = exact_float_expressions;
        function.late_bound_owner_fields = late_bound_owner_fields;
        let scalar_fields = late_bound_split_owner_scalar_field_plans(
            function,
            &hot_fields,
            owner_field_sites.as_slice(),
        );
        function.late_bound_owner_fields.extend(scalar_fields);
        let nonself_fields = late_bound_split_owner_nonself_field_plans(
            function,
            &hot_fields,
            owner_field_sites.as_slice(),
            module_identity.module_name.as_str(),
        );
        function.late_bound_owner_fields.extend(nonself_fields);
    }
    validate_module_plan_v3_against_lowered_module(&plan, lowered_module)
        .map_err(ExactIntBranchV3Error::Emit)?;
    let emission = emit_mechanical_plan_v3(&plan).map_err(ExactIntBranchV3Error::Emit)?;
    Ok(ExactIntBranchV3Artifacts { plan, emission })
}

fn late_bound_split_owner_scalar_field_plans(
    function: &FunctionOptimizationPlanV3,
    hot_fields: &HashMap<InstrId, u64>,
    owner_field_sites: &[(RuntimeFunctionId, LateBoundOwnerFieldSpecializationPlan)],
) -> Vec<LateBoundOwnerFieldSpecializationPlan> {
    let mut selected_sources = function
        .late_bound_owner_fields
        .iter()
        .map(|plan| plan.source)
        .collect::<HashSet<_>>();
    let mut selected = Vec::new();

    for RegionPlan { inputs, .. } in &function.regions {
        for input in inputs {
            if input.value.rep != Rep::PyObjectBorrowed {
                continue;
            }
            let RegionInputSource::IndexedField {
                source,
                owner_type,
                attr_name,
                expected_index,
                ..
            } = &input.source
            else {
                continue;
            };
            if selected_sources.contains(source)
                || hot_fields.get(source).copied().unwrap_or_default() < 8
            {
                continue;
            }
            let Some(anchor) = owner_field_sites
                .iter()
                .map(|(_, site)| site)
                .filter(|site| {
                    site.owner_type == *owner_type
                        && site.attr_name == *attr_name
                        && matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. })
                })
                .min_by_key(|site| site.cell_index)
            else {
                continue;
            };
            selected_sources.insert(*source);
            selected.push(LateBoundOwnerFieldSpecializationPlan {
                source: *source,
                access: IndexedFieldAccessKind::Load,
                owner_type: owner_type.clone(),
                attr_name: attr_name.clone(),
                storage: LateBoundOwnerFieldStorage::SplitDict {
                    expected_index: *expected_index,
                },
                cell_index: anchor.cell_index,
                reason:
                    "profiled scalar region reuses a same-owner split-field late-binding guard cell"
                        .to_string(),
            });
        }
    }

    selected
}

fn late_bound_split_owner_nonself_field_plans(
    function: &FunctionOptimizationPlanV3,
    hot_fields: &HashMap<InstrId, u64>,
    owner_field_sites: &[(RuntimeFunctionId, LateBoundOwnerFieldSpecializationPlan)],
    module_name: &str,
) -> Vec<LateBoundOwnerFieldSpecializationPlan> {
    const MAX_NONSELF_FIELDS_PER_FUNCTION: usize = 8;
    const MAX_POLYMORPHIC_NONSELF_OWNERS_PER_FIELD: usize = 5;

    let mut layouts_by_source = HashMap::new();
    for field in &function.indexed_fields {
        layouts_by_source
            .entry((field.source, field.access))
            .or_insert_with(Vec::new)
            .push(field);
    }
    let selected_sources = function
        .late_bound_owner_fields
        .iter()
        .map(|plan| plan.source)
        .collect::<HashSet<_>>();
    let mut unique_candidates = Vec::new();
    let mut polymorphic_candidates = Vec::new();

    for fields in layouts_by_source.values() {
        let Some(field) = fields.first().copied() else {
            continue;
        };
        let hot_count = hot_fields.get(&field.source).copied().unwrap_or_default();
        if hot_count < 8
            || selected_sources.contains(&field.source)
            || field.owner_type.module_name != module_name
        {
            continue;
        }

        if fields.len() > 1
            && (field.access != IndexedFieldAccessKind::Load
                || fields.len() > MAX_POLYMORPHIC_NONSELF_OWNERS_PER_FIELD
                || fields.iter().any(|candidate| {
                    candidate.owner_type.module_name != module_name
                        || candidate.attr_name != field.attr_name
                }))
        {
            continue;
        }

        let mut owners = HashSet::new();
        let mut plans = Vec::with_capacity(fields.len());
        for candidate in fields {
            if !owners.insert(&candidate.owner_type) {
                plans.clear();
                break;
            }
            let Some(anchor) = owner_field_sites
                .iter()
                .map(|(_, site)| site)
                .filter(|site| {
                    site.owner_type == candidate.owner_type
                        && site.attr_name == candidate.attr_name
                        && matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. })
                })
                .min_by_key(|site| site.cell_index)
            else {
                plans.clear();
                break;
            };
            plans.push(LateBoundOwnerFieldSpecializationPlan {
                source: candidate.source,
                access: candidate.access,
                owner_type: candidate.owner_type.clone(),
                attr_name: candidate.attr_name.clone(),
                storage: LateBoundOwnerFieldStorage::SplitDict {
                    expected_index: candidate.expected_index,
                },
                cell_index: anchor.cell_index,
                reason: if fields.len() == 1 {
                    "profiled unique-owner field reuses an existing split-field late-binding guard cell"
                        .to_string()
                } else {
                    "profiled polymorphic field reuses independently guarded exact-owner split-field cells"
                        .to_string()
                },
            });
        }
        if plans.len() != fields.len() {
            continue;
        }

        if plans.len() == 1 {
            unique_candidates.push((
                hot_count,
                plans.pop().expect("unique owner candidate has one plan"),
            ));
        } else {
            plans.sort_by_key(|plan| (plan.owner_type.qualname.clone(), plan.cell_index));
            polymorphic_candidates.push((hot_count, field.source, field.access, plans));
        }
    }

    unique_candidates
        .sort_by_key(|(hot_count, plan)| (std::cmp::Reverse(*hot_count), plan.source, plan.access));
    unique_candidates.truncate(MAX_NONSELF_FIELDS_PER_FUNCTION);

    let remaining_sources = MAX_NONSELF_FIELDS_PER_FUNCTION - unique_candidates.len();
    polymorphic_candidates.sort_by_key(|(hot_count, source, access, _)| {
        (std::cmp::Reverse(*hot_count), *source, *access)
    });
    polymorphic_candidates.truncate(remaining_sources);

    unique_candidates
        .into_iter()
        .map(|(_, plan)| plan)
        .chain(
            polymorphic_candidates
                .into_iter()
                .flat_map(|(_, _, _, plans)| plans),
        )
        .collect()
}

fn function_uses_generator_resume_state(function: &BlockPyFunction<BlockPyModuleShape>) -> bool {
    function.storage_layout.as_ref().is_some_and(|layout| {
        layout
            .generator_control_slot(soac_core::block_py::GeneratorControlRole::ProgramCounter)
            .is_some()
    })
}

fn literal_same_module_class_ancestors(
    module: &BlockPyModule<BlockPyModuleShape>,
    known_owners: &HashSet<String>,
) -> HashMap<String, HashSet<String>> {
    struct Collector<'a> {
        module: &'a BlockPyModule<BlockPyModuleShape>,
        known_owners: &'a HashSet<String>,
        parents: HashMap<String, Vec<String>>,
        ambiguous: HashSet<String>,
    }

    impl Visit<InstrBlockPy> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if let InstrBlockPy::Store(store) = expr
                && self.known_owners.contains(store.name.id_str())
                && let InstrBlockPy::Call(call) = store.value.as_ref()
                && let InstrBlockPy::Load(helper) = call.func.as_ref()
                && helper.name.id_str() == format!("_dp_define_class_{}", store.name.id_str())
                && call.keywords.is_empty()
                && call.args.len() == 4
                && let CallArgPositional::Positional(InstrBlockPy::Tuple(bases)) = &call.args[2]
                && let CallArgPositional::Positional(prepare) = &call.args[3]
                && codegen_runtime_name_value_v3(self.module, prepare) == Some(RuntimeName::None)
            {
                let parents = bases
                    .values
                    .iter()
                    .map(|base| {
                        let InstrBlockPy::Load(base) = base else {
                            return None;
                        };
                        self.known_owners
                            .contains(base.name.id_str())
                            .then(|| base.name.id_str().to_string())
                    })
                    .collect::<Option<Vec<_>>>();
                if let Some(parents) = parents {
                    let owner = store.name.id_str().to_string();
                    if self.parents.insert(owner.clone(), parents).is_some() {
                        self.ambiguous.insert(owner);
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        module,
        known_owners,
        parents: HashMap::new(),
        ambiguous: HashSet::new(),
    };
    for function in &module.callable_defs {
        if function.scope.scope_kind == CallableScopeKind::Module {
            collector.visit_fn(function);
        }
    }
    for owner in collector.ambiguous {
        collector.parents.remove(&owner);
    }

    let mut ancestry = HashMap::new();
    for owner in collector.parents.keys() {
        let mut ancestors = HashSet::new();
        let mut pending = collector.parents.get(owner).cloned().unwrap_or_default();
        while let Some(parent) = pending.pop() {
            if parent == *owner || !ancestors.insert(parent.clone()) {
                continue;
            }
            if let Some(next) = collector.parents.get(&parent) {
                pending.extend(next.iter().cloned());
            }
        }
        ancestry.insert(owner.clone(), ancestors);
    }
    ancestry
}

/// Deterministically enumerates structurally proven class-method field sites.
///
/// The same immutable catalog is used when selecting profiled plans and when
/// allocating module-owned late-binding cells, so precompiled code never embeds
/// a process-local owner address or depends on profile enumeration order.
pub fn late_bound_owner_field_site_catalog(
    module: &BlockPyModule<BlockPyModuleShape>,
    module_name: &str,
) -> Vec<(RuntimeFunctionId, LateBoundOwnerFieldSpecializationPlan)> {
    #[derive(Clone)]
    struct OwnerShape {
        qualname: String,
        declared_slots: Option<HashSet<String>>,
    }

    struct ClassCollector<'a> {
        module: &'a BlockPyModule<BlockPyModuleShape>,
        namespace_name: &'a str,
        storage_layout: Option<&'a soac_core::block_py::StorageLayout>,
        constant_locals: HashMap<LocalLocation, String>,
        methods: HashMap<String, RuntimeFunctionId>,
        declared_slots: Option<Option<HashSet<String>>>,
        qualname: Option<String>,
    }

    impl ClassCollector<'_> {
        fn constant_string<'a>(&'a self, expr: &InstrBlockPy) -> Option<&'a str> {
            if let Some(value) = codegen_constant_string_value_v3(self.module, expr) {
                return Some(value);
            }
            let location = match expr {
                InstrBlockPy::Load(load) => load.name.local_location()?,
                InstrBlockPy::TakeOperand(take) => {
                    // Read the same literal evidence without changing the consuming
                    // operation or treating a source local as an operand owner.
                    let soac_core::block_py::OperandLocation::Local(location) =
                        take.validate_resolved(self.storage_layout?).ok()?
                    else {
                        return None;
                    };
                    location
                }
                _ => return None,
            };
            self.constant_locals.get(&location).map(String::as_str)
        }
    }

    impl Visit<InstrBlockPy> for ClassCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if let InstrBlockPy::SetItem(op) = expr
                && let InstrBlockPy::Load(namespace) = op.value.as_ref()
                && matches!(namespace.name.location, NameLocation::Local(_))
                && namespace.name.id_str() == self.namespace_name
                && let Some(name) = self.constant_string(&op.index).map(str::to_string)
            {
                if name == "__qualname__" {
                    self.qualname = self
                        .constant_string(op.replacement.as_ref())
                        .map(str::to_string);
                } else if name == "__slots__" {
                    let slots = match op.replacement.as_ref() {
                        InstrBlockPy::Tuple(tuple) => tuple
                            .values
                            .iter()
                            .map(|value| self.constant_string(value).map(str::to_string))
                            .collect::<Option<HashSet<_>>>(),
                        value => self
                            .constant_string(value)
                            .map(|value| HashSet::from([value.to_string()])),
                    };
                    if self.declared_slots.is_none() {
                        self.declared_slots = Some(slots);
                    } else {
                        self.declared_slots = Some(None);
                    }
                } else {
                    let method = match op.replacement.as_ref() {
                        InstrBlockPy::MakeFunctionWithClosure(function) => {
                            Some(function.function_id())
                        }
                        _ => None,
                    };
                    if let Some(function_id) = method {
                        self.methods.insert(name.to_string(), function_id);
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    struct ClassConstantCollector<'a> {
        module: &'a BlockPyModule<BlockPyModuleShape>,
        values: HashMap<LocalLocation, Option<String>>,
    }

    impl Visit<InstrBlockPy> for ClassConstantCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if let InstrBlockPy::Store(store) = expr
                && let NameLocation::Local(location) = store.name.location
            {
                let value =
                    codegen_constant_string_value_v3(self.module, &store.value).map(str::to_string);
                match self.values.entry(location) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(value);
                    }
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        entry.insert(None);
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    let functions_by_id = module
        .callable_defs
        .iter()
        .map(|function| (function.function_id, function))
        .collect::<HashMap<_, _>>();
    let mut owners_by_function = HashMap::new();
    let mut owners_by_qualname = HashMap::new();
    for class_function in &module.callable_defs {
        if class_function.scope.scope_kind != CallableScopeKind::Class {
            continue;
        }
        let Some(&namespace_index) = class_function.params.positional_param_indices().first()
        else {
            continue;
        };
        let mut constants = ClassConstantCollector {
            module,
            values: HashMap::new(),
        };
        constants.visit_fn(class_function);
        let mut collector = ClassCollector {
            module,
            namespace_name: class_function.params.params[namespace_index].name.as_str(),
            storage_layout: class_function.storage_layout.as_ref(),
            constant_locals: constants
                .values
                .into_iter()
                .filter_map(|(location, value)| value.map(|value| (location, value)))
                .collect(),
            methods: HashMap::new(),
            declared_slots: None,
            qualname: None,
        };
        collector.visit_fn(class_function);
        let Some(owner_qualname) = collector.qualname else {
            continue;
        };
        if owner_qualname.is_empty() || owner_qualname.contains("<locals>") {
            continue;
        }
        let declared_slots = match collector.declared_slots {
            Some(Some(slots)) => Some(slots),
            Some(None) => continue,
            None => None,
        };
        owners_by_qualname.insert(owner_qualname.clone(), declared_slots.clone());
        for (method_name, function_id) in collector.methods {
            let Some(function) = functions_by_id.get(&function_id) else {
                continue;
            };
            if function.names.qualname != format!("{owner_qualname}.{method_name}") {
                continue;
            }
            owners_by_function.insert(
                function_id,
                OwnerShape {
                    qualname: owner_qualname.clone(),
                    declared_slots: declared_slots.clone(),
                },
            );
        }
    }

    struct AccessCollector<'a> {
        module: &'a BlockPyModule<BlockPyModuleShape>,
        module_name: &'a str,
        function_id: RuntimeFunctionId,
        receiver_name: &'a str,
        owner: &'a OwnerShape,
        sites: Vec<(RuntimeFunctionId, LateBoundOwnerFieldSpecializationPlan)>,
    }

    impl AccessCollector<'_> {
        fn collect(
            &mut self,
            source: InstrId,
            access: IndexedFieldAccessKind,
            receiver: &InstrBlockPy,
            attr: &InstrBlockPy,
        ) {
            let InstrBlockPy::Load(receiver) = receiver else {
                return;
            };
            if !matches!(receiver.name.location, NameLocation::Local(_))
                || receiver.name.id_str() != self.receiver_name
            {
                return;
            }
            let Some(attr_name) = codegen_constant_string_value_v3(self.module, attr) else {
                return;
            };
            let storage = match &self.owner.declared_slots {
                Some(slots) if slots.contains(attr_name) => LateBoundOwnerFieldStorage::ObjectSlot,
                Some(_) => return,
                None => LateBoundOwnerFieldStorage::SplitDict { expected_index: 0 },
            };
            self.sites.push((
                self.function_id,
                LateBoundOwnerFieldSpecializationPlan {
                    source,
                    access,
                    owner_type: IndexedFieldOwnerType {
                        module_name: self.module_name.to_string(),
                        qualname: self.owner.qualname.clone(),
                    },
                    attr_name: attr_name.to_string(),
                    storage,
                    cell_index: 0,
                    reason: "profiled class-method receiver uses a late-bound owner field"
                        .to_string(),
                },
            ));
        }
    }

    impl Visit<InstrBlockPy> for AccessCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            match expr {
                InstrBlockPy::GetAttr(op) => self.collect(
                    op.semantic_instr_id(),
                    IndexedFieldAccessKind::Load,
                    &op.value,
                    &op.attr,
                ),
                InstrBlockPy::SetAttr(op) => self.collect(
                    op.semantic_instr_id(),
                    IndexedFieldAccessKind::Store,
                    &op.value,
                    &op.attr,
                ),
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut sites = Vec::new();
    for function in &module.callable_defs {
        let Some(owner) = owners_by_function.get(&function.function_id) else {
            continue;
        };
        let Some(&parameter_index) = function.params.positional_param_indices().first() else {
            continue;
        };
        let receiver_name = function.params.params[parameter_index].name.as_str();
        let mut collector = AccessCollector {
            module,
            module_name,
            function_id: function.function_id,
            receiver_name,
            owner,
            sites: Vec::new(),
        };
        collector.visit_fn(function);
        sites.extend(collector.sites);
    }
    let split_owners = owners_by_qualname
        .iter()
        .filter(|(_, slots)| slots.is_none())
        .map(|(owner, _)| owner.clone())
        .collect::<HashSet<_>>();
    let ancestry = literal_same_module_class_ancestors(module, &split_owners);
    let written_fields = sites
        .iter()
        .filter(|(_, site)| {
            site.access == IndexedFieldAccessKind::Store
                && matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. })
        })
        .map(|(_, site)| (site.owner_type.qualname.clone(), site.attr_name.clone()))
        .collect::<HashSet<_>>();
    let mut existing_anchors = sites
        .iter()
        .filter(|(_, site)| matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. }))
        .map(|(_, site)| (site.owner_type.qualname.clone(), site.attr_name.clone()))
        .collect::<HashSet<_>>();
    let mut inherited = Vec::new();
    let mut descendants = ancestry.keys().collect::<Vec<_>>();
    descendants.sort_unstable();
    for descendant in descendants {
        let ancestors = &ancestry[descendant];
        for (function_id, site) in &sites {
            if !matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. })
                || !ancestors.contains(site.owner_type.qualname.as_str())
                || !written_fields.iter().any(|(owner, attr)| {
                    attr == &site.attr_name
                        && (owner == descendant || ancestors.contains(owner.as_str()))
                })
                || !existing_anchors.insert((descendant.clone(), site.attr_name.clone()))
            {
                continue;
            }
            let mut anchor = site.clone();
            anchor.owner_type.qualname = descendant.clone();
            anchor.reason =
                "literal same-module split descendant reuses one inherited owner-field cell"
                    .to_string();
            inherited.push((*function_id, anchor));
        }
    }
    sites.extend(inherited);
    sites.sort_by_key(|(function_id, site)| {
        (
            function_id.local_function_id().as_u32(),
            site.source,
            site.access,
            site.owner_type.qualname.clone(),
            site.attr_name.clone(),
        )
    });
    sites.dedup_by(|left, right| {
        left.0 == right.0
            && left.1.source == right.1.source
            && left.1.access == right.1.access
            && left.1.owner_type == right.1.owner_type
            && left.1.attr_name == right.1.attr_name
    });
    for (index, (_, site)) in sites.iter_mut().enumerate() {
        site.cell_index = u32::try_from(index).expect("too many late-bound owner-field sites");
    }
    sites
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
            identity_tables: identity_tables_for_single_module(&module),
            module,
            functions: vec![FunctionPlanRequest {
                direct_calls: Vec::new(),
                exact_list_items: Vec::new(),
                indexed_fields: Vec::new(),
                opaque_fused_iterations: Vec::new(),
                indexed_globals: Vec::new(),
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

fn validate_module_plan_v3_against_lowered_module(
    plan: &ModuleOptimizationPlanV3,
    lowered_module: &BlockPyModule<BlockPyModuleShape>,
) -> Result<(), MechanicalEmitError> {
    for planned_function in &plan.functions {
        let local_function_id = planned_function.function.function.local_function_id();
        let lowered_function = lowered_module
            .callable_defs
            .iter()
            .find(|function| {
                function.function_id.local_function_id().as_u32() == local_function_id.as_u32()
            })
            .ok_or_else(|| {
                MechanicalEmitError::EmissionMismatch(format!(
                    "function {} has no matching lowered function",
                    planned_function.function.function
                ))
            })?;
        validate_function_plan_v3_against_lowered_function(
            plan,
            planned_function,
            lowered_module,
            lowered_function,
        )?;
    }
    Ok(())
}

fn validate_function_plan_v3_against_lowered_function(
    plan: &ModuleOptimizationPlanV3,
    planned_function: &FunctionOptimizationPlanV3,
    lowered_module: &BlockPyModule<BlockPyModuleShape>,
    lowered_function: &BlockPyFunction<BlockPyModuleShape>,
) -> Result<(), MechanicalEmitError> {
    if !planned_function.exact_float_expressions.is_empty() {
        let lowered_expressions = lowered_exact_float_expressions_by_instr_v3(lowered_function);
        for selected in &planned_function.exact_float_expressions {
            let Some(lowered) = lowered_expressions.get(&selected.source) else {
                return Err(MechanicalEmitError::EmissionMismatch(format!(
                    "function {} exact-float expression at {} has no replay-safe lowered expression",
                    planned_function.function.function, selected.source
                )));
            };
            if lowered.operations != selected.operations
                || lowered.leaf_sources != selected.leaf_sources
            {
                return Err(MechanicalEmitError::EmissionMismatch(format!(
                    "function {} exact-float expression at {} does not match its lowered arithmetic tree",
                    planned_function.function.function, selected.source
                )));
            }
        }
    }
    if !planned_function.direct_calls.is_empty() {
        let lowered_calls = lowered_calls_by_instr_v3(lowered_module, lowered_function)?;
        for direct_call in &planned_function.direct_calls {
            validate_direct_call_plan_against_lowered_function(
                direct_call,
                planned_function,
                lowered_function,
                &lowered_calls,
            )?;
        }
    }
    if !planned_function.exact_list_items.is_empty() {
        let lowered_accesses = lowered_item_accesses_by_instr_v3(lowered_function);
        for item in &planned_function.exact_list_items {
            validate_exact_list_item_plan_against_lowered_function(
                item,
                planned_function,
                lowered_function,
                &lowered_accesses,
            )?;
        }
    }
    if !planned_function.indexed_fields.is_empty() {
        let lowered_accesses =
            lowered_field_accesses_by_instr_v3(lowered_module, lowered_function)?;
        for indexed_field in &planned_function.indexed_fields {
            validate_indexed_field_plan_against_lowered_function(
                indexed_field,
                planned_function,
                lowered_function,
                &lowered_accesses,
            )?;
        }
    }
    if !planned_function.late_bound_owner_fields.is_empty() {
        let lowered_accesses =
            lowered_field_accesses_by_instr_v3(lowered_module, lowered_function)?;
        for selected in &planned_function.late_bound_owner_fields {
            let Some(lowered) = lowered_accesses.get(&selected.source) else {
                return Err(MechanicalEmitError::EmissionMismatch(format!(
                    "function {} late-bound owner-field at {} has no lowered attribute access",
                    planned_function.function.function, selected.source
                )));
            };
            if lowered.access != selected.access || lowered.attr_name != selected.attr_name {
                return Err(MechanicalEmitError::EmissionMismatch(format!(
                    "function {} late-bound owner-field at {} does not match lowered access or attribute",
                    planned_function.function.function, selected.source
                )));
            }
        }
    }
    if !planned_function.indexed_globals.is_empty() {
        let lowered_accesses = lowered_global_accesses_by_instr_v3(lowered_function);
        for indexed_global in &planned_function.indexed_globals {
            if indexed_global.module_name != plan.module.module_name {
                return Err(MechanicalEmitError::EmissionMismatch(format!(
                    "function {} indexed-global at {} names module {}, expected {}",
                    planned_function.function.function,
                    indexed_global.source,
                    indexed_global.module_name,
                    plan.module.module_name
                )));
            }
            validate_indexed_global_plan_against_lowered_function(
                indexed_global,
                planned_function,
                lowered_function,
                &lowered_accesses,
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoweredCallAccessV3 {
    method_name: Option<String>,
}

fn lowered_calls_by_instr_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> Result<HashMap<InstrId, LoweredCallAccessV3>, MechanicalEmitError> {
    struct Collector<'a> {
        module: &'a BlockPyModule<BlockPyModuleShape>,
        calls: HashMap<InstrId, LoweredCallAccessV3>,
        error: Option<MechanicalEmitError>,
    }

    impl Visit<InstrBlockPy> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if self.error.is_some() {
                return;
            }
            if let InstrBlockPy::Call(call) = expr
                && let Some(source) = call.try_semantic_instr_id()
            {
                let method_name = match call.func.as_ref() {
                    InstrBlockPy::GetAttr(getattr) => {
                        let Some(method_name) =
                            codegen_constant_string_value_v3(self.module, getattr.attr.as_ref())
                        else {
                            self.error = Some(MechanicalEmitError::EmissionMismatch(format!(
                                "method-call source {source} has non-constant lowered attribute"
                            )));
                            return;
                        };
                        Some(method_name.to_string())
                    }
                    _ => None,
                };
                self.calls
                    .insert(source, LoweredCallAccessV3 { method_name });
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        module,
        calls: HashMap::new(),
        error: None,
    };
    collector.visit_fn(function);
    if let Some(error) = collector.error {
        return Err(error);
    }
    Ok(collector.calls)
}

fn validate_direct_call_plan_against_lowered_function(
    plan: &DirectCallSpecializationPlan,
    planned_function: &FunctionOptimizationPlanV3,
    lowered_function: &BlockPyFunction<BlockPyModuleShape>,
    lowered_calls: &HashMap<InstrId, LoweredCallAccessV3>,
) -> Result<(), MechanicalEmitError> {
    if !lowered_calls.contains_key(&plan.source) {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} direct-call at {}, but lowered function {} has no call with that source",
            planned_function.function.function, plan.source, lowered_function.function_id
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoweredFieldAccessV3 {
    access: IndexedFieldAccessKind,
    attr_name: String,
}

fn lowered_field_accesses_by_instr_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> Result<HashMap<InstrId, LoweredFieldAccessV3>, MechanicalEmitError> {
    struct Collector<'a> {
        module: &'a BlockPyModule<BlockPyModuleShape>,
        accesses: HashMap<InstrId, LoweredFieldAccessV3>,
        error: Option<MechanicalEmitError>,
    }

    impl Collector<'_> {
        fn attr_name_for_source(&mut self, source: InstrId, attr: &InstrBlockPy) -> Option<String> {
            match codegen_constant_string_value_v3(self.module, attr) {
                Some(attr_name) => Some(attr_name.to_string()),
                None => {
                    self.error = Some(MechanicalEmitError::EmissionMismatch(format!(
                        "indexed-field source {source} expected constant attribute, but lowered attr is not a module string constant"
                    )));
                    None
                }
            }
        }
    }

    impl Visit<InstrBlockPy> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrBlockPy::GetAttr(op) => {
                    let source = op.semantic_instr_id();
                    if let Some(attr_name) = self.attr_name_for_source(source, op.attr.as_ref()) {
                        self.accesses.insert(
                            source,
                            LoweredFieldAccessV3 {
                                access: IndexedFieldAccessKind::Load,
                                attr_name,
                            },
                        );
                    }
                }
                InstrBlockPy::SetAttr(op) => {
                    let source = op.semantic_instr_id();
                    if let Some(attr_name) = self.attr_name_for_source(source, op.attr.as_ref()) {
                        self.accesses.insert(
                            source,
                            LoweredFieldAccessV3 {
                                access: IndexedFieldAccessKind::Store,
                                attr_name,
                            },
                        );
                    }
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        module,
        accesses: HashMap::new(),
        error: None,
    };
    collector.visit_fn(function);
    if let Some(error) = collector.error {
        return Err(error);
    }
    Ok(collector.accesses)
}

fn validate_indexed_field_plan_against_lowered_function(
    plan: &IndexedFieldSpecializationPlan,
    planned_function: &FunctionOptimizationPlanV3,
    lowered_function: &BlockPyFunction<BlockPyModuleShape>,
    lowered_accesses: &HashMap<InstrId, LoweredFieldAccessV3>,
) -> Result<(), MechanicalEmitError> {
    let Some(lowered) = lowered_accesses.get(&plan.source) else {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} indexed-field {:?} {} at {}, but lowered function {} has no GetAttr/SetAttr with that source",
            planned_function.function.function,
            plan.access,
            plan.attr_name,
            plan.source,
            lowered_function.function_id
        )));
    };
    if lowered.access != plan.access {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} indexed-field {:?} for {}, but lowered instruction is {:?}",
            planned_function.function.function, plan.access, plan.source, lowered.access
        )));
    }
    if lowered.attr_name != plan.attr_name {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} indexed-field attr {:?} for {}, but lowered instruction uses {:?}",
            planned_function.function.function, plan.attr_name, plan.source, lowered.attr_name
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoweredItemAccessV3 {
    access: ExactListItemAccessKind,
}

fn lowered_item_accesses_by_instr_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> HashMap<InstrId, LoweredItemAccessV3> {
    struct Collector {
        accesses: HashMap<InstrId, LoweredItemAccessV3>,
    }

    impl Visit<InstrBlockPy> for Collector {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            match expr {
                InstrBlockPy::GetItem(op) => {
                    self.accesses.insert(
                        op.semantic_instr_id(),
                        LoweredItemAccessV3 {
                            access: ExactListItemAccessKind::Get,
                        },
                    );
                }
                InstrBlockPy::SetItem(op) => {
                    self.accesses.insert(
                        op.semantic_instr_id(),
                        LoweredItemAccessV3 {
                            access: ExactListItemAccessKind::Set,
                        },
                    );
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        accesses: HashMap::new(),
    };
    collector.visit_fn(function);
    collector.accesses
}

fn validate_exact_list_item_plan_against_lowered_function(
    plan: &ExactListItemSpecializationPlan,
    planned_function: &FunctionOptimizationPlanV3,
    lowered_function: &BlockPyFunction<BlockPyModuleShape>,
    lowered_accesses: &HashMap<InstrId, LoweredItemAccessV3>,
) -> Result<(), MechanicalEmitError> {
    let Some(lowered) = lowered_accesses.get(&plan.source) else {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} exact-list item {:?} {:?} at {}, but lowered function {} has no getitem/setitem with that source",
            planned_function.function.function,
            plan.access,
            plan.shape,
            plan.source,
            lowered_function.function_id
        )));
    };
    if lowered.access != plan.access {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} exact-list item {:?} for {}, but lowered instruction is {:?}",
            planned_function.function.function, plan.access, plan.source, lowered.access
        )));
    }
    if !matches!(
        (plan.shape, plan.access),
        (ExactListItemShape::ExactListExactInt, _)
            | (
                ExactListItemShape::ExactTupleExactInt,
                ExactListItemAccessKind::Get
            )
    ) {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} exact-list item shape {:?} with access {:?} for {}, but codegen only supports exact-list get/set and exact-tuple get",
            planned_function.function.function, plan.shape, plan.access, plan.source
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoweredGlobalAccessV3 {
    access: IndexedGlobalAccessKind,
    name: String,
    slot: u32,
}

fn lowered_global_accesses_by_instr_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> HashMap<InstrId, LoweredGlobalAccessV3> {
    struct Collector {
        accesses: HashMap<InstrId, LoweredGlobalAccessV3>,
    }

    impl Visit<InstrBlockPy> for Collector {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            match expr {
                InstrBlockPy::Load(op) => {
                    if let NameLocation::Global(slot) = op.name.location {
                        let Some(source) = op.try_semantic_instr_id() else {
                            expr.visit_children(self);
                            return;
                        };
                        self.accesses.insert(
                            source,
                            LoweredGlobalAccessV3 {
                                access: IndexedGlobalAccessKind::Load,
                                name: op.name.id_str().to_string(),
                                slot: slot.slot(),
                            },
                        );
                    }
                }
                InstrBlockPy::Store(op) => {
                    if let NameLocation::Global(slot) = op.name.location {
                        let Some(source) = op.try_semantic_instr_id() else {
                            expr.visit_children(self);
                            return;
                        };
                        self.accesses.insert(
                            source,
                            LoweredGlobalAccessV3 {
                                access: IndexedGlobalAccessKind::Store,
                                name: op.name.id_str().to_string(),
                                slot: slot.slot(),
                            },
                        );
                    }
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        accesses: HashMap::new(),
    };
    collector.visit_fn(function);
    collector.accesses
}

fn validate_indexed_global_plan_against_lowered_function(
    plan: &IndexedGlobalSpecializationPlan,
    planned_function: &FunctionOptimizationPlanV3,
    lowered_function: &BlockPyFunction<BlockPyModuleShape>,
    lowered_accesses: &HashMap<InstrId, LoweredGlobalAccessV3>,
) -> Result<(), MechanicalEmitError> {
    let Some(lowered) = lowered_accesses.get(&plan.source) else {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} indexed-global {:?} {}.{} at {}, but lowered function {} has no global load/store with that source",
            planned_function.function.function,
            plan.access,
            plan.module_name,
            plan.name,
            plan.source,
            lowered_function.function_id
        )));
    };
    if lowered.access != plan.access {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} indexed-global {:?} for {}, but lowered instruction is {:?}",
            planned_function.function.function, plan.access, plan.source, lowered.access
        )));
    }
    if lowered.name != plan.name {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} indexed-global name {:?} for {}, but lowered instruction uses {:?}",
            planned_function.function.function, plan.name, plan.source, lowered.name
        )));
    }
    if lowered.slot != plan.expected_index {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} indexed-global slot {} for {}, but lowered instruction uses global slot {}",
            planned_function.function.function, plan.expected_index, plan.source, lowered.slot
        )));
    }
    Ok(())
}

fn function_plan_identity_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> FunctionPlanIdentity {
    FunctionPlanIdentity {
        function: SerializedFunctionId::new(
            SerializedModuleId::new(0),
            LocalFunctionId::new(function.function_id.local_function_id().as_u32()),
        ),
        debug_name: Some(function.names.qualname.clone()),
    }
}

fn exact_float_expression_plan_for_instr_v3(
    expr: &InstrBlockPy,
    evidence: Option<&FunctionProfileEvidence>,
) -> Option<ExactFloatExpressionSpecializationPlan> {
    fn collect(
        expr: &InstrBlockPy,
        evidence: Option<&FunctionProfileEvidence>,
        operations: &mut Vec<ExactFloatExpressionOperationPlan>,
        leaf_sources: &mut Vec<InstrId>,
    ) -> Option<()> {
        match expr {
            InstrBlockPy::BinOp(op)
                if matches!(op.kind, BinOpKind::Add | BinOpKind::Sub | BinOpKind::Mul) =>
            {
                let source = op.try_semantic_instr_id()?;
                if let Some(evidence) = evidence {
                    let expected = pack_binary_shape(ExactTypeTag::Float, ExactTypeTag::Float);
                    if !evidence
                        .operator_specializations
                        .get(&source)
                        .is_some_and(|shapes| shapes.contains(&expected))
                    {
                        return None;
                    }
                }
                collect(op.left.as_ref(), evidence, operations, leaf_sources)?;
                collect(op.right.as_ref(), evidence, operations, leaf_sources)?;
                operations.push(ExactFloatExpressionOperationPlan {
                    source,
                    kind: op.kind,
                });
                Some(())
            }
            InstrBlockPy::Load(op)
                if matches!(
                    op.name.location,
                    NameLocation::Local(_) | NameLocation::Constant(_)
                ) =>
            {
                leaf_sources.push(op.try_semantic_instr_id()?);
                Some(())
            }
            _ => None,
        }
    }

    let InstrBlockPy::BinOp(root) = expr else {
        return None;
    };
    let mut operations = Vec::new();
    let mut leaf_sources = Vec::new();
    collect(expr, evidence, &mut operations, &mut leaf_sources)?;
    if operations.len() < 2 {
        return None;
    }
    Some(ExactFloatExpressionSpecializationPlan {
        source: root.try_semantic_instr_id()?,
        operations,
        leaf_sources,
        reason: "profiled exact-float arithmetic selected a maximal multi-operation expression"
            .to_string(),
    })
}

fn exact_float_expression_plans_from_profile_evidence_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
    evidence: &FunctionProfileEvidence,
) -> Vec<ExactFloatExpressionSpecializationPlan> {
    struct Collector<'a> {
        evidence: &'a FunctionProfileEvidence,
        plans: Vec<ExactFloatExpressionSpecializationPlan>,
    }

    impl Visit<InstrBlockPy> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if let Some(plan) = exact_float_expression_plan_for_instr_v3(expr, Some(self.evidence))
            {
                self.plans.push(plan);
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        evidence,
        plans: Vec::new(),
    };
    collector.visit_fn(function);
    collector.plans.sort_by_key(|plan| plan.source);
    collector.plans
}

fn lowered_exact_float_expressions_by_instr_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> HashMap<InstrId, ExactFloatExpressionSpecializationPlan> {
    struct Collector {
        plans: HashMap<InstrId, ExactFloatExpressionSpecializationPlan>,
    }

    impl Visit<InstrBlockPy> for Collector {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if let Some(plan) = exact_float_expression_plan_for_instr_v3(expr, None) {
                self.plans.insert(plan.source, plan);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        plans: HashMap::new(),
    };
    collector.visit_fn(function);
    collector.plans
}

fn exact_list_item_requests_from_profile_evidence_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
    evidence: &FunctionProfileEvidence,
) -> (Vec<ExactListItemPlanRequest>, Vec<PlanDiagnostic>) {
    struct Collector<'a> {
        evidence: &'a FunctionProfileEvidence,
        requests: Vec<ExactListItemPlanRequest>,
        diagnostics: Vec<PlanDiagnostic>,
    }

    impl Collector<'_> {
        fn collect_item(
            &mut self,
            source: InstrId,
            access: ExactListItemAccessKind,
            shapes_by_instr: &HashMap<InstrId, Vec<u64>>,
            counter_kind: &str,
        ) {
            let Some(shapes) = shapes_by_instr.get(&source) else {
                return;
            };
            for shape in shapes {
                match *shape {
                    EXACT_LIST_EXACT_INT_ITEM_SHAPE_TAG => {
                        self.requests.push(ExactListItemPlanRequest {
                            source,
                            access,
                            shape: ExactListItemShape::ExactListExactInt,
                            reason: format!(
                                "profiled {counter_kind} selected exact-list/exact-int item specialization"
                            ),
                        });
                    }
                    EXACT_TUPLE_EXACT_INT_ITEM_SHAPE_TAG
                        if access == ExactListItemAccessKind::Get =>
                    {
                        self.requests.push(ExactListItemPlanRequest {
                            source,
                            access,
                            shape: ExactListItemShape::ExactTupleExactInt,
                            reason: format!(
                                "profiled {counter_kind} selected exact-tuple/exact-int item specialization"
                            ),
                        });
                    }
                    0 => {}
                    other => self.diagnostics.push(PlanDiagnostic {
                        source: Some(source),
                        message: format!(
                            "v3 exact-list item declined unsupported {counter_kind} shape {other}"
                        ),
                    }),
                }
            }
        }
    }

    impl Visit<InstrBlockPy> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            match expr {
                InstrBlockPy::GetItem(op) => {
                    self.collect_item(
                        op.semantic_instr_id(),
                        ExactListItemAccessKind::Get,
                        &self.evidence.getitem_specializations,
                        "getitem_hot_shapes",
                    );
                }
                InstrBlockPy::SetItem(op) => {
                    self.collect_item(
                        op.semantic_instr_id(),
                        ExactListItemAccessKind::Set,
                        &self.evidence.setitem_specializations,
                        "setitem_hot_shapes",
                    );
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        evidence,
        requests: Vec::new(),
        diagnostics: Vec::new(),
    };
    collector.visit_fn(function);
    (collector.requests, collector.diagnostics)
}

fn indexed_global_requests_from_module_key_evidence_v3(
    module_name: &str,
    function: &BlockPyFunction<BlockPyModuleShape>,
    evidence_store: &ProfileEvidenceStore,
) -> Vec<IndexedGlobalPlanRequest> {
    struct Collector<'a> {
        module_name: &'a str,
        evidence_store: &'a ProfileEvidenceStore,
        requests: Vec<IndexedGlobalPlanRequest>,
    }

    impl Collector<'_> {
        fn collect_name(
            &mut self,
            source: InstrId,
            access: IndexedGlobalAccessKind,
            name: &soac_core::block_py::ResolvedName,
        ) {
            let NameLocation::Global(slot) = name.location else {
                return;
            };
            let Some(specializations) = self
                .evidence_store
                .global_index_specializations_for_name(self.module_name, name.id_str())
            else {
                return;
            };
            for specialization in specializations {
                if specialization.expected_index != slot.slot() {
                    continue;
                }
                self.requests.push(IndexedGlobalPlanRequest {
                    source,
                    access,
                    module_name: specialization.module_name.clone(),
                    name: specialization.name.clone(),
                    expected_index: specialization.expected_index,
                    reason: "profiled module_keys selected this indexed-global slot for a lowered global access".to_string(),
                });
            }
        }
    }

    impl Visit<InstrBlockPy> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            match expr {
                InstrBlockPy::Load(op) => {
                    let Some(source) = op.try_semantic_instr_id() else {
                        expr.visit_children(self);
                        return;
                    };
                    self.collect_name(source, IndexedGlobalAccessKind::Load, &op.name);
                }
                InstrBlockPy::Store(op) => {
                    let Some(source) = op.try_semantic_instr_id() else {
                        expr.visit_children(self);
                        return;
                    };
                    self.collect_name(source, IndexedGlobalAccessKind::Store, &op.name);
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        module_name,
        evidence_store,
        requests: Vec::new(),
    };
    collector.visit_fn(function);
    collector.requests
}

fn indexed_field_requests_from_type_key_evidence_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
    lowered_module: &BlockPyModule<BlockPyModuleShape>,
    evidence_store: &ProfileEvidenceStore,
) -> Vec<IndexedFieldPlanRequest> {
    struct Collector<'a> {
        lowered_module: &'a BlockPyModule<BlockPyModuleShape>,
        evidence_store: &'a ProfileEvidenceStore,
        requests: Vec<IndexedFieldPlanRequest>,
    }

    impl Collector<'_> {
        fn collect_attr(
            &mut self,
            source: InstrId,
            access: IndexedFieldAccessKind,
            attr_expr: &InstrBlockPy,
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

    impl Visit<InstrBlockPy> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            match expr {
                InstrBlockPy::GetAttr(op) => {
                    self.collect_attr(
                        op.semantic_instr_id(),
                        IndexedFieldAccessKind::Load,
                        op.attr.as_ref(),
                    );
                }
                InstrBlockPy::SetAttr(op) => {
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
    module: &'a BlockPyModule<BlockPyModuleShape>,
    expr: &InstrBlockPy,
) -> Option<&'a str> {
    let InstrBlockPy::Load(load) = expr else {
        return None;
    };
    let NameLocation::Constant(constant_index) = load.name.location else {
        return None;
    };
    module_constant_string_value_v3(module, constant_index)
}

fn codegen_runtime_name_value_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    expr: &InstrBlockPy,
) -> Option<RuntimeName> {
    let InstrBlockPy::Load(load) = expr else {
        return None;
    };
    load.name.location.runtime_name_id().or_else(|| {
        let NameLocation::Constant(constant_index) = load.name.location else {
            return None;
        };
        let ConstantExpr::RuntimeName(runtime_name) =
            module.module_constants.get(constant_index as usize)?
        else {
            return None;
        };
        Some(*runtime_name)
    })
}

fn runtime_protocol_method_for_call_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    call: &Call<InstrBlockPy>,
) -> Option<(RuntimeName, &'static str)> {
    if !call.keywords.is_empty() || call.args.len() != 1 {
        return None;
    }
    let CallArgPositional::Positional(_) = call.args.first()? else {
        return None;
    };
    match codegen_runtime_name_value_v3(module, call.func.as_ref())? {
        RuntimeName::Iter => Some((RuntimeName::Iter, "__iter__")),
        RuntimeName::Next => Some((RuntimeName::Next, "__next__")),
        _ => None,
    }
}

fn exact_stop_iteration_runtime_match_for_instr_id_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    source: InstrId,
) -> bool {
    struct Finder<'a> {
        module: &'a BlockPyModule<BlockPyModuleShape>,
        source: InstrId,
        matches: bool,
    }

    impl Visit<InstrBlockPy> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if self.matches {
                return;
            }
            if let InstrBlockPy::Call(call) = expr
                && call.try_semantic_instr_id() == Some(self.source)
                && call.keywords.is_empty()
                && let [
                    CallArgPositional::Positional(_),
                    CallArgPositional::Positional(expected),
                ] = call.args.as_slice()
                && codegen_runtime_name_value_v3(self.module, call.func.as_ref())
                    == Some(RuntimeName::ExceptionMatches)
                && codegen_runtime_name_value_v3(self.module, expected)
                    == Some(RuntimeName::StopIteration)
            {
                self.matches = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        module,
        source,
        matches: false,
    };
    finder.visit_fn(function);
    finder.matches
}

fn compiler_owned_eager_comprehension_call_for_instr_id_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
    target: &BlockPyFunction<BlockPyModuleShape>,
    source: InstrId,
) -> bool {
    let expected_prefix = match target.names.display_name.as_str() {
        "<listcomp>" => "_dp_listcomp_",
        "<setcomp>" => "_dp_setcomp_",
        "<dictcomp>" => "_dp_dictcomp_",
        _ => return false,
    };
    let Some(generated_suffix) = target.names.bind_name.strip_prefix(expected_prefix) else {
        return false;
    };
    if generated_suffix.is_empty()
        || !generated_suffix.bytes().all(|byte| byte.is_ascii_digit())
        || target.lowered_kind() != &FunctionKind::Function
        || target.execution_mode() != FunctionExecutionMode::Jit
    {
        return false;
    }
    let [parameter] = target.params.params.as_slice() else {
        return false;
    };
    if !matches!(parameter.kind, ParamKind::Any | ParamKind::PosOnly)
        || parameter.has_default
        || !parameter.name.starts_with("_dp_iter_")
    {
        return false;
    }

    struct Finder<'a> {
        source: InstrId,
        target: &'a BlockPyFunction<BlockPyModuleShape>,
        generated_locals: HashSet<LocalLocation>,
        called_local: Option<LocalLocation>,
    }

    impl Visit<InstrBlockPy> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            match expr {
                InstrBlockPy::Store(store)
                    if store.name.id_str() == self.target.names.bind_name
                        && let Some(local) = store.name.local_location()
                        && let InstrBlockPy::MakeFunctionWithClosure(make) =
                            store.value.as_ref()
                        && make.function_id() == self.target.function_id =>
                {
                    self.generated_locals.insert(local);
                }
                InstrBlockPy::Call(call)
                    if call.try_semantic_instr_id() == Some(self.source)
                        && call.keywords.is_empty()
                        && matches!(call.args.as_slice(), [CallArgPositional::Positional(_)])
                        && let InstrBlockPy::Load(callee) = call.func.as_ref()
                        && callee.name.id_str() == self.target.names.bind_name =>
                {
                    self.called_local = callee.name.local_location();
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        source,
        target,
        generated_locals: HashSet::new(),
        called_local: None,
    };
    finder.visit_fn(function);
    finder
        .called_local
        .is_some_and(|local| finder.generated_locals.contains(&local))
}

#[derive(Clone, Debug)]
enum DirectCallSourceCallee {
    Function,
    Method {
        method_name: String,
    },
    MethodWithDynamicName,
    RuntimeProtocolMethod {
        runtime_name: RuntimeName,
        method_name: String,
    },
}

fn module_constant_string_value_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    constant_index: u32,
) -> Option<&str> {
    let ConstantExpr::Literal(literal) = module.module_constants.get(constant_index as usize)?
    else {
        return None;
    };
    let Literal::StringLiteral(literal) = literal.as_literal() else {
        return None;
    };
    Some(literal.value.as_str())
}

fn direct_call_requests_from_evidence_v3(
    lowered_module: &BlockPyModule<BlockPyModuleShape>,
    module_identity: &ModulePlanIdentity,
    function: &BlockPyFunction<BlockPyModuleShape>,
    evidence_store: &ProfileEvidenceStore,
    target_index: &DirectCallTargetIndex,
    identity_builder: &mut OptimizationPlanV3IdentityBuilder,
) -> (Vec<DirectCallPlanRequest>, Vec<PlanDiagnostic>) {
    let mut requests = Vec::new();
    let mut diagnostics = Vec::new();
    let current_module = ModuleContentId::new(
        module_identity.module_name.clone(),
        module_identity.source_hash,
    );
    let mut entries = evidence_store
        .persistent_call_target_specializations_for_runtime_function_v3(
            module_identity.module_name.as_str(),
            module_identity.source_hash,
            function.function_id,
        )
        .into_iter()
        .collect::<Vec<_>>();
    entries.sort_by_key(|(source, _)| *source);
    for (source, targets) in entries {
        if exact_stop_iteration_runtime_match_for_instr_id_v3(lowered_module, function, source) {
            diagnostics.push(PlanDiagnostic {
                source: Some(source),
                message: "v3 direct-call declined exact runtime StopIteration match: preserve its live-guarded generic vectorcall path".to_string(),
            });
            continue;
        }
        let source_callee = match direct_call_source_callee_for_instr_id_v3(
            lowered_module,
            function,
            source,
        ) {
            DirectCallSourceCallee::MethodWithDynamicName => {
                diagnostics.push(PlanDiagnostic {
                    source: Some(source),
                    message: "v3 direct-call declined method source: lowered attribute name is not a constant string".to_string(),
                });
                continue;
            }
            source_callee => source_callee,
        };
        let mut targets = targets;
        targets.sort();
        targets.dedup();
        for target in targets {
            let serialized_target = identity_builder.function_id_for_persistent(target.clone());
            let Some(target_entry) = target_index.entry(&target) else {
                diagnostics.push(PlanDiagnostic {
                    source: Some(source),
                    message: format!(
                        "v3 direct-call declined target {serialized_target}: target function is missing from cached modules"
                    ),
                });
                continue;
            };
            let target_function = &target_entry.function;
            if target_entry.module == current_module
                && compiler_owned_eager_comprehension_call_for_instr_id_v3(
                    function,
                    target_function,
                    source,
                )
            {
                diagnostics.push(PlanDiagnostic {
                    source: Some(source),
                    message: format!(
                        "v3 direct-call declined target {serialized_target}: compiler-owned eager comprehensions use their guarded callable path"
                    ),
                });
                continue;
            }
            if target_function.execution_mode() != FunctionExecutionMode::Jit {
                diagnostics.push(PlanDiagnostic {
                    source: Some(source),
                    message: format!(
                        "v3 direct-call declined target {serialized_target}: target function is not JIT lowered"
                    ),
                });
                continue;
            }
            if target_function.lowered_kind() != &FunctionKind::Function {
                diagnostics.push(PlanDiagnostic {
                    source: Some(source),
                    message: format!(
                        "v3 direct-call declined target {serialized_target}: generator-like targets require public factory semantics, not direct resume-body calls"
                    ),
                });
                continue;
            }
            let callee = match &source_callee {
                DirectCallSourceCallee::Function => DirectCallCallee::Function,
                DirectCallSourceCallee::Method { method_name } => DirectCallCallee::Method {
                    method_name: method_name.clone(),
                },
                DirectCallSourceCallee::RuntimeProtocolMethod {
                    runtime_name,
                    method_name,
                } => DirectCallCallee::RuntimeProtocolMethod {
                    runtime_name: *runtime_name,
                    method_name: method_name.clone(),
                },
                DirectCallSourceCallee::MethodWithDynamicName => unreachable!(
                    "dynamic method sources should be rejected before direct-call planning"
                ),
            };
            let implicit_positional_arg_count = match &callee {
                DirectCallCallee::Function => {
                    usize::from(is_constructor_entry_function(target_function))
                }
                DirectCallCallee::Method { .. } => 1,
                DirectCallCallee::RuntimeProtocolMethod { .. } => 0,
            };
            let arg_plan = match direct_call_arg_plan_for_instr_id_v3(
                function,
                source,
                target_function,
                implicit_positional_arg_count,
            ) {
                Some(Ok(arg_plan)) => arg_plan,
                Some(Err(reason)) => {
                    diagnostics.push(PlanDiagnostic {
                        source: Some(source),
                        message: format!(
                            "v3 direct-call declined target {serialized_target}: {reason}"
                        ),
                    });
                    continue;
                }
                None => {
                    diagnostics.push(PlanDiagnostic {
                        source: Some(source),
                        message: format!(
                            "v3 direct-call declined target {serialized_target}: source instruction is not a lowered call"
                        ),
                    });
                    continue;
                }
            };
            if is_constructor_entry_function(target_function)
                && arg_plan
                    .sources
                    .iter()
                    .any(|source| matches!(source, DirectCallArgSource::DefaultSentinel))
            {
                diagnostics.push(PlanDiagnostic {
                    source: Some(source),
                    message: format!(
                        "v3 direct-call declined target {serialized_target}: constructor entries do not yet refresh default arguments"
                    ),
                });
                continue;
            }
            let inline_candidate = direct_call_inline_candidate_v3(
                lowered_module,
                &current_module,
                function,
                source,
                target_entry,
                &callee,
                &arg_plan,
            );
            requests.push(DirectCallPlanRequest {
                source,
                target: serialized_target,
                callee: callee.clone(),
                arg_plan,
                body: CallBodyPlanRequest::with_inline_candidate(inline_candidate),
                reason: match callee {
                    DirectCallCallee::Function => {
                        "profiled call_hot_targets selected this function with validated ordinary-call arguments".to_string()
                    }
                    DirectCallCallee::Method { .. } => {
                        "profiled call_hot_targets selected this method with validated receiver-method arguments".to_string()
                    }
                    DirectCallCallee::RuntimeProtocolMethod { .. } => {
                        "profiled call_hot_targets selected this runtime protocol method with validated receiver-method arguments".to_string()
                    }
                },
            });
            identity_builder
                .add_debug_name(serialized_target, target_function.names.qualname.clone());
        }
    }
    (requests, diagnostics)
}

fn direct_call_source_callee_for_instr_id_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    source: InstrId,
) -> DirectCallSourceCallee {
    struct Finder<'a> {
        module: &'a BlockPyModule<BlockPyModuleShape>,
        source: InstrId,
        result: Option<DirectCallSourceCallee>,
    }

    impl Visit<InstrBlockPy> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if self.result.is_some() {
                return;
            }
            if let InstrBlockPy::Call(call) = expr
                && call.try_semantic_instr_id() == Some(self.source)
            {
                self.result = if let Some((runtime_name, method_name)) =
                    runtime_protocol_method_for_call_v3(self.module, call)
                {
                    Some(DirectCallSourceCallee::RuntimeProtocolMethod {
                        runtime_name,
                        method_name: method_name.to_string(),
                    })
                } else if let InstrBlockPy::GetAttr(getattr) = call.func.as_ref() {
                    match codegen_constant_string_value_v3(self.module, getattr.attr.as_ref()) {
                        Some(method_name) => Some(DirectCallSourceCallee::Method {
                            method_name: method_name.to_string(),
                        }),
                        None => Some(DirectCallSourceCallee::MethodWithDynamicName),
                    }
                } else {
                    Some(DirectCallSourceCallee::Function)
                };
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        module,
        source,
        result: None,
    };
    finder.visit_fn(function);
    finder.result.unwrap_or(DirectCallSourceCallee::Function)
}

fn direct_call_arg_plan_for_instr_id_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
    source: InstrId,
    target_function: &BlockPyFunction<BlockPyModuleShape>,
    implicit_positional_arg_count: usize,
) -> Option<std::result::Result<DirectCallArgPlan, String>> {
    struct Finder<'a> {
        source: InstrId,
        target_function: &'a BlockPyFunction<BlockPyModuleShape>,
        implicit_positional_arg_count: usize,
        result: Option<std::result::Result<DirectCallArgPlan, String>>,
    }

    impl Visit<InstrBlockPy> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            if self.result.is_some() {
                return;
            }
            if let InstrBlockPy::Call(call) = expr
                && call.try_semantic_instr_id() == Some(self.source)
            {
                self.result = Some(direct_call_arg_plan_from_call_v3(
                    call,
                    self.target_function,
                    self.implicit_positional_arg_count,
                ));
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        source,
        target_function,
        implicit_positional_arg_count,
        result: None,
    };
    finder.visit_fn(function);
    finder.result
}

fn direct_call_inline_candidate_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    current_module: &ModuleContentId,
    function: &BlockPyFunction<BlockPyModuleShape>,
    source: InstrId,
    target: &DirectCallTargetEntry,
    callee: &DirectCallCallee,
    arg_plan: &DirectCallArgPlan,
) -> bool {
    let Some((source_block, return_target, call)) =
        inline_call_and_return_target_for_instr_id_v3(function, source)
    else {
        return false;
    };
    let target_function = &target.function;
    if !crate::passes::InlineHandledContext::for_call_site(source_block).can_inline(target_function)
    {
        return false;
    }
    if target_function.names.fn_name == "__init__" {
        return false;
    }
    if !call_inline_signature_candidate_v3(call, target_function, arg_plan) {
        return false;
    }
    match callee {
        DirectCallCallee::Function => direct_call_inline_body_buildable_v3(
            module,
            current_module,
            function,
            return_target,
            call,
            target,
            arg_plan,
        )
        .is_ok(),
        DirectCallCallee::Method { .. } => direct_method_inline_body_buildable_v3(
            module,
            current_module,
            function,
            return_target,
            call,
            target,
            arg_plan,
        )
        .is_ok(),
        DirectCallCallee::RuntimeProtocolMethod { .. } => {
            direct_runtime_protocol_method_inline_body_buildable_v3(
                module,
                current_module,
                function,
                return_target,
                call,
                target,
                arg_plan,
            )
            .is_ok()
        }
    }
}

enum InlineCallReturnTargetV3 {
    StoreTo(ResolvedName),
    Discard,
}

fn inline_call_and_return_target_for_instr_id_v3(
    function: &BlockPyFunction<BlockPyModuleShape>,
    source: InstrId,
) -> Option<(
    &soac_core::block_py::Block<InstrBlockPy>,
    InlineCallReturnTargetV3,
    &Call<InstrBlockPy>,
)> {
    for block in &function.blocks {
        for instr in &block.body {
            match instr {
                InstrBlockPy::Store(store) => {
                    let InstrBlockPy::Call(call) = store.value.as_ref() else {
                        continue;
                    };
                    if call.try_semantic_instr_id() == Some(source) {
                        return Some((
                            block,
                            InlineCallReturnTargetV3::StoreTo(store.name.clone()),
                            call,
                        ));
                    }
                }
                InstrBlockPy::Call(call) if call.try_semantic_instr_id() == Some(source) => {
                    return Some((block, InlineCallReturnTargetV3::Discard, call));
                }
                _ => {}
            }
        }
        if let BlockTerm::Return(InstrBlockPy::Call(call)) = &block.term
            && call.try_semantic_instr_id() == Some(source)
        {
            return Some((block, InlineCallReturnTargetV3::Discard, call));
        }
    }
    None
}

fn direct_call_inline_body_buildable_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    current_module: &ModuleContentId,
    function: &BlockPyFunction<BlockPyModuleShape>,
    return_target: InlineCallReturnTargetV3,
    call: &Call<InstrBlockPy>,
    target: &DirectCallTargetEntry,
    arg_plan: &DirectCallArgPlan,
) -> Result<(), InlineUnsupportedReason> {
    let mut caller = function.clone();
    let continuation = caller.name_gen.next_block_name();
    let return_target = match return_target {
        InlineCallReturnTargetV3::StoreTo(target) => target,
        InlineCallReturnTargetV3::Discard => {
            try_allocate_codegen_stack_temp(&mut caller, "inline_discard_result")
                .map_err(|_| InlineUnsupportedReason::MissingCallerStorageLayout)?
                .resolved_name()
        }
    };
    let values = direct_call_inline_values_for_function_call(call, &target.function)?;
    let bindings =
        bind_v3_direct_call_inline_values(&target.function, arg_plan, values.as_slice())?;
    if target.module == *current_module {
        build_direct_call_inline_fragment_to_target(
            &mut caller,
            &target.function,
            continuation,
            &bindings,
            return_target,
        )?;
    } else {
        let mut caller_constants = module.module_constants.clone();
        build_cross_module_direct_call_inline_fragment_to_target(
            &mut caller,
            &mut caller_constants,
            &target.function,
            target.module_constants.as_slice(),
            continuation,
            &bindings,
            return_target,
        )?;
    }
    Ok(())
}

fn direct_method_inline_body_buildable_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    current_module: &ModuleContentId,
    function: &BlockPyFunction<BlockPyModuleShape>,
    return_target: InlineCallReturnTargetV3,
    call: &Call<InstrBlockPy>,
    target: &DirectCallTargetEntry,
    arg_plan: &DirectCallArgPlan,
) -> Result<(), InlineUnsupportedReason> {
    let mut caller = function.clone();
    let continuation = caller.name_gen.next_block_name();
    let return_target = match return_target {
        InlineCallReturnTargetV3::StoreTo(target) => target,
        InlineCallReturnTargetV3::Discard => {
            try_allocate_codegen_stack_temp(&mut caller, "inline_discard_result")
                .map_err(|_| InlineUnsupportedReason::MissingCallerStorageLayout)?
                .resolved_name()
        }
    };
    let InstrBlockPy::GetAttr(get_attr) = call.func.as_ref() else {
        return Err(InlineUnsupportedReason::UnsupportedCallTarget);
    };
    let values = direct_call_inline_values_for_method_call(call, get_attr.value.as_ref().clone())?;
    let bindings =
        bind_v3_direct_call_inline_values(&target.function, arg_plan, values.as_slice())?;
    if target.module == *current_module {
        build_direct_call_inline_fragment_to_target(
            &mut caller,
            &target.function,
            continuation,
            &bindings,
            return_target,
        )?;
    } else {
        let mut caller_constants = module.module_constants.clone();
        build_cross_module_direct_call_inline_fragment_to_target(
            &mut caller,
            &mut caller_constants,
            &target.function,
            target.module_constants.as_slice(),
            continuation,
            &bindings,
            return_target,
        )?;
    }
    Ok(())
}

fn direct_runtime_protocol_method_inline_body_buildable_v3(
    module: &BlockPyModule<BlockPyModuleShape>,
    current_module: &ModuleContentId,
    function: &BlockPyFunction<BlockPyModuleShape>,
    return_target: InlineCallReturnTargetV3,
    call: &Call<InstrBlockPy>,
    target: &DirectCallTargetEntry,
    arg_plan: &DirectCallArgPlan,
) -> Result<(), InlineUnsupportedReason> {
    if runtime_protocol_method_for_call_v3(module, call).is_none() {
        return Err(InlineUnsupportedReason::UnsupportedCallTarget);
    }
    let [CallArgPositional::Positional(receiver)] = call.args.as_slice() else {
        return Err(InlineUnsupportedReason::UnsupportedCallTarget);
    };
    let values = vec![receiver.clone()];
    let bindings =
        bind_v3_direct_call_inline_values(&target.function, arg_plan, values.as_slice())?;
    let mut caller = function.clone();
    let continuation = caller.name_gen.next_block_name();
    let return_target = match return_target {
        InlineCallReturnTargetV3::StoreTo(target) => target,
        InlineCallReturnTargetV3::Discard => {
            try_allocate_codegen_stack_temp(&mut caller, "inline_discard_result")
                .map_err(|_| InlineUnsupportedReason::MissingCallerStorageLayout)?
                .resolved_name()
        }
    };
    if target.module == *current_module {
        build_direct_call_inline_fragment_to_target(
            &mut caller,
            &target.function,
            continuation,
            &bindings,
            return_target,
        )?;
    } else {
        let mut caller_constants = module.module_constants.clone();
        build_cross_module_direct_call_inline_fragment_to_target(
            &mut caller,
            &mut caller_constants,
            &target.function,
            target.module_constants.as_slice(),
            continuation,
            &bindings,
            return_target,
        )?;
    }
    Ok(())
}

fn direct_call_inline_values_for_function_call(
    call: &Call<InstrBlockPy>,
    target_function: &BlockPyFunction<BlockPyModuleShape>,
) -> Result<Vec<InstrBlockPy>, InlineUnsupportedReason> {
    let mut values = Vec::with_capacity(
        call.args.len() + usize::from(is_constructor_entry_function(target_function)),
    );
    if is_constructor_entry_function(target_function) {
        values.push(call.func.as_ref().clone());
    }
    values.extend(positional_inline_arg_values(call.args.as_slice())?);
    Ok(values)
}

fn direct_call_inline_values_for_method_call(
    call: &Call<InstrBlockPy>,
    receiver: InstrBlockPy,
) -> Result<Vec<InstrBlockPy>, InlineUnsupportedReason> {
    let mut values = Vec::with_capacity(call.args.len() + 1);
    values.push(receiver);
    values.extend(positional_inline_arg_values(call.args.as_slice())?);
    Ok(values)
}

fn positional_inline_arg_values(
    args: &[CallArgPositional<InstrBlockPy>],
) -> Result<Vec<InstrBlockPy>, InlineUnsupportedReason> {
    args.iter()
        .map(|arg| match arg {
            CallArgPositional::Positional(value) => Ok(value.clone()),
            CallArgPositional::Starred(_) => Err(InlineUnsupportedReason::StarredArguments),
        })
        .collect()
}

fn bind_v3_direct_call_inline_values(
    callee: &BlockPyFunction<BlockPyModuleShape>,
    arg_plan: &DirectCallArgPlan,
    values: &[InstrBlockPy],
) -> Result<InlineValueBindings, InlineUnsupportedReason> {
    if arg_plan.sources.len() != callee.params.len() {
        return Err(InlineUnsupportedReason::ArityMismatch {
            expected: callee.params.len(),
            actual: arg_plan.sources.len(),
        });
    }

    let mut bindings = InlineValueBindings::new();
    for (param, source) in callee.params.iter().zip(&arg_plan.sources) {
        let value = match (&param.kind, source) {
            (ParamKind::PosOnly | ParamKind::Any, DirectCallArgSource::Provided(index)) => values
                .get(*index as usize)
                .cloned()
                .ok_or(InlineUnsupportedReason::ArityMismatch {
                    expected: *index as usize + 1,
                    actual: values.len(),
                })?,
            (ParamKind::VarArg, DirectCallArgSource::PackedRest { start }) => {
                let rest = values
                    .get(*start as usize..)
                    .ok_or(InlineUnsupportedReason::ArityMismatch {
                        expected: *start as usize,
                        actual: values.len(),
                    })?
                    .to_vec();
                InstrBlockPy::Tuple(Tuple::new(rest))
            }
            (_, DirectCallArgSource::DefaultSentinel) => {
                return Err(InlineUnsupportedReason::ArityMismatch {
                    expected: values.len() + 1,
                    actual: values.len(),
                });
            }
            (_, _) => {
                return Err(InlineUnsupportedReason::UnsupportedParameterKind {
                    name: param.name.clone(),
                    kind: param.kind,
                });
            }
        };
        let location = blockpy_parameter_local_location(callee, &param.name)?;
        bindings.insert(location, value);
    }
    Ok(bindings)
}

fn blockpy_parameter_local_location(
    function: &BlockPyFunction<BlockPyModuleShape>,
    name: &str,
) -> Result<LocalLocation, InlineUnsupportedReason> {
    let layout = function
        .storage_layout
        .as_ref()
        .ok_or(InlineUnsupportedReason::MissingCalleeStorageLayout)?;
    let Some(slot) = layout
        .stack_slots()
        .iter()
        .position(|slot_name| slot_name == name)
    else {
        return Err(InlineUnsupportedReason::MissingParameterLocal(
            name.to_string(),
        ));
    };
    Ok(LocalLocation(
        u32::try_from(slot).expect("parameter stack slot index should fit in u32"),
    ))
}

fn call_inline_signature_candidate_v3(
    call: &Call<InstrBlockPy>,
    target_function: &BlockPyFunction<BlockPyModuleShape>,
    arg_plan: &DirectCallArgPlan,
) -> bool {
    if !call.keywords.is_empty()
        || call
            .args
            .iter()
            .any(|arg| matches!(arg, CallArgPositional::Starred(_)))
    {
        return false;
    }
    let Some(storage_layout) = &target_function.storage_layout else {
        return false;
    };
    if arg_plan.sources.len() != target_function.params.len() {
        return false;
    }
    for (param, source) in target_function.params.iter().zip(&arg_plan.sources) {
        match (param.kind, source) {
            (ParamKind::PosOnly | ParamKind::Any, DirectCallArgSource::Provided(_))
            | (ParamKind::VarArg, DirectCallArgSource::PackedRest { .. }) => {}
            _ => return false,
        }
        if !storage_layout
            .stack_slots()
            .iter()
            .any(|name| name == &param.name)
        {
            return false;
        }
    }
    true
}

fn direct_call_arg_plan_from_call_v3(
    call: &Call<InstrBlockPy>,
    target_function: &BlockPyFunction<BlockPyModuleShape>,
    implicit_positional_arg_count: usize,
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

    let has_vararg = target_function
        .params
        .iter()
        .any(|param| matches!(param.kind, ParamKind::VarArg));
    for param in target_function.params.iter() {
        if matches!(param.kind, ParamKind::KwArg) {
            return Err(format!(
                "target parameter kind {:?} is not supported",
                param.kind
            ));
        }
    }

    let explicit_positional_arg_count = call
        .args
        .iter()
        .filter(|arg| matches!(arg, CallArgPositional::Positional(_)))
        .count();
    let provided_positional_arg_count =
        implicit_positional_arg_count + explicit_positional_arg_count;
    let accepted_positional_arg_count = target_function
        .params
        .iter()
        .filter(|param| matches!(param.kind, ParamKind::PosOnly | ParamKind::Any))
        .count();
    if !has_vararg && provided_positional_arg_count > accepted_positional_arg_count {
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
            ParamKind::VarArg => {
                sources.push(DirectCallArgSource::PackedRest {
                    start: next_provided_arg
                        .try_into()
                        .map_err(|_| "too many positional arguments for v3 arg plan")?,
                });
                next_provided_arg = provided_positional_arg_count;
            }
            ParamKind::KwArg => unreachable!(
                "unsupported variadic keyword params should be rejected before planning direct-call args"
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

pub fn optimize_modules_v3_from_raw_evidence<'a>(
    evidence_store: &ProfileEvidenceStore,
    module_inputs: impl IntoIterator<Item = ModuleOptimizationInput<'a>>,
) -> Result<OptimizeModulesV3Output> {
    let module_inputs = module_inputs.into_iter().collect::<Vec<_>>();
    let target_index = DirectCallTargetIndex::from_modules(module_inputs.as_slice());
    let mut evidence_store = evidence_store.clone();
    for module_input in &module_inputs {
        evidence_store.record_runtime_module_target(
            module_input.module.module_name_gen.runtime_module_id(),
            module_input.identity.module_name.as_str(),
            module_input.identity.source_hash,
        );
    }
    let mut output = OptimizeModulesV3Output::default();
    for module_input in &module_inputs {
        match optimize_module_v3_from_raw_evidence_with_target_index(
            &evidence_store,
            &module_input.identity,
            module_input.module,
            module_input.strict,
            &target_index,
        )? {
            Some(planned) => output.modules.push(planned),
            None => output.skipped += 1,
        }
    }
    Ok(output)
}

pub fn optimize_module_v3_from_raw_evidence(
    evidence_store: &ProfileEvidenceStore,
    module_identity: ModulePlanIdentity,
    module: &BlockPyModule<BlockPyModuleShape>,
    strict: bool,
) -> Result<Option<PlannedModuleV3>> {
    let target_index = DirectCallTargetIndex::from_current_module(&module_identity, module);
    optimize_module_v3_from_raw_evidence_with_target_index(
        evidence_store,
        &module_identity,
        module,
        strict,
        &target_index,
    )
}

fn optimize_module_v3_from_raw_evidence_with_target_index(
    evidence_store: &ProfileEvidenceStore,
    module_identity: &ModulePlanIdentity,
    module: &BlockPyModule<BlockPyModuleShape>,
    strict: bool,
    target_index: &DirectCallTargetIndex,
) -> Result<Option<PlannedModuleV3>> {
    if !counter_evidence_matches_module_v3(evidence_store, module_identity, strict)? {
        return Ok(None);
    }
    let catalog = AlternativeCatalog::default_v3();
    let artifacts = plan_and_emit_module_v3_from_raw_evidence_with_target_index(
        &catalog,
        module_identity,
        module,
        evidence_store,
        target_index,
    )?;
    Ok(Some(PlannedModuleV3 {
        identity: module_identity.clone(),
        artifacts,
    }))
}

fn counter_evidence_matches_module_v3(
    evidence_store: &ProfileEvidenceStore,
    module_identity: &ModulePlanIdentity,
    strict: bool,
) -> Result<bool> {
    match evidence_store.module_source_hash(module_identity.module_name.as_str()) {
        Some(source_hash) if source_hash == module_identity.source_hash => Ok(true),
        Some(source_hash) => anyhow::bail!(
            "counter dump source hash for module {} is 0x{source_hash:016x}, but cached BlockPy module has 0x{:016x}",
            module_identity.module_name,
            module_identity.source_hash
        ),
        None if strict => anyhow::bail!(
            "counter dump does not contain module {}",
            module_identity.module_name
        ),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_resume_state_requires_a_producer_control_role() {
        let module = soac_lowering::lower_python_to_blockpy_for_testing(
            "def owner(_dp_pc):\n    def read():\n        return _dp_pc\n    return read\n\ndef suspended(_dp_pc):\n    yield _dp_pc\n",
        )
        .expect("source control spellings should lower")
        .blockpy_module;
        for name in ["owner", "read"] {
            let function = module
                .callable_defs
                .iter()
                .find(|function| function.names.display_name == name)
                .unwrap();
            assert!(
                !function_uses_generator_resume_state(function),
                "ordinary source locals/captures are not a suspended activation"
            );
        }
        let suspended = module
            .callable_defs
            .iter()
            .find(|function| function.names.display_name == "suspended")
            .unwrap();
        assert!(function_uses_generator_resume_state(suspended));
        let layout = suspended.storage_layout.as_ref().unwrap();
        let location = layout
            .generator_control_slot(soac_core::block_py::GeneratorControlRole::ProgramCounter)
            .unwrap();
        assert_ne!(
            layout.preserved_slot(location.slot()).unwrap().logical_name,
            "_dp_pc",
            "the real control was allocated away from the source parameter"
        );
    }

    #[test]
    fn late_bound_owner_field_catalog_finds_static_slot_and_split_methods() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class Point:
    __slots__ = ("value",)

    def read(self):
        return self.value

    def write(self, value):
        self.value = value

class Record:
    def read(self):
        return self.value

    def write(self, value):
        self.value = value

class Decorated:
    @staticmethod
    def read(instance):
        return instance.value

class Inherited(Point):
    __slots__ = ()

    def read(self):
        return self.value

def make_dynamic():
    class Dynamic:
        def read(self):
            return self.value

    return Dynamic
"#,
        )
        .expect("class owner-field fixture should lower")
        .blockpy_module;
        let catalog = late_bound_owner_field_site_catalog(&lowered, "pkg.mod");
        let actual = catalog
            .iter()
            .map(|(_, plan)| (plan.owner_type.qualname.as_str(), plan.access, plan.storage))
            .collect::<Vec<_>>();
        assert!(
            actual.contains(&(
                "Point",
                IndexedFieldAccessKind::Load,
                LateBoundOwnerFieldStorage::ObjectSlot,
            )),
            "slot owner read was not selected: {actual:?}"
        );
        assert!(
            actual.contains(&(
                "Point",
                IndexedFieldAccessKind::Store,
                LateBoundOwnerFieldStorage::ObjectSlot,
            )),
            "slot owner write was not selected: {actual:?}"
        );
        assert!(
            actual.contains(&(
                "Record",
                IndexedFieldAccessKind::Load,
                LateBoundOwnerFieldStorage::SplitDict { expected_index: 0 },
            )),
            "split owner read was not selected: {actual:?}"
        );
        assert!(
            actual.contains(&(
                "Record",
                IndexedFieldAccessKind::Store,
                LateBoundOwnerFieldStorage::SplitDict { expected_index: 0 },
            )),
            "split owner write was not selected: {actual:?}"
        );
        assert!(
            catalog
                .iter()
                .enumerate()
                .all(|(index, (_, plan))| plan.cell_index == index as u32),
            "owner cells must have stable dense catalog indices",
        );
        assert!(
            actual.iter().all(|(owner, _, _)| {
                !matches!(*owner, "Decorated" | "Inherited") && !owner.contains("<locals>")
            }),
            "decorated, inherited, and dynamic owner bindings must not be selected: {actual:?}",
        );
    }

    #[test]
    fn late_bound_owner_field_catalog_requires_validated_constant_operand_transport() {
        use soac_contracts::{DefinitionKind, SourceIdentity};
        use soac_core::block_py::{CompleteFunctionDefinition, OperandLocation, StoreLifetime};

        let source = "class Point:\n    __slots__ = ('value',)\n    def read(self):\n        return self.value\n";
        let module = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("original slotted class should lower")
            .blockpy_module;
        let class_index = module
            .callable_defs
            .iter()
            .position(|function| function.scope.scope_kind == CallableScopeKind::Class)
            .expect("source class must retain its namespace body");
        let class = &module.callable_defs[class_index];
        let stores = class
            .blocks
            .iter()
            .enumerate()
            .flat_map(|(block_index, block)| {
                block.body.iter().filter_map(move |instr| {
                    let InstrBlockPy::Store(store) = instr else {
                        return None;
                    };
                    Some((block_index, store))
                })
            })
            .filter(|(_, store)| {
                codegen_constant_string_value_v3(&module, &store.value) == Some("Point")
            })
            .collect::<Vec<_>>();
        assert_eq!(stores.len(), 1, "qualname must have one literal producer");
        let (producer_block, producer) = stores[0];
        assert!(matches!(producer.lifetime, StoreLifetime::Operand { .. }));
        let operand = producer.name.local_location().unwrap();
        let layout = class.storage_layout.as_ref().unwrap();

        struct ConstantTake<'a> {
            operand: LocalLocation,
            layout: &'a StorageLayout,
            count: usize,
        }
        impl Visit<InstrBlockPy> for ConstantTake<'_> {
            fn visit_instr(&mut self, expr: &InstrBlockPy) {
                if let InstrBlockPy::TakeOperand(take) = expr
                    && take.name.local_location() == Some(self.operand)
                {
                    assert_eq!(
                        take.validate_resolved(self.layout).unwrap(),
                        OperandLocation::Local(self.operand),
                    );
                    self.count += 1;
                }
                expr.visit_children(self);
            }
        }
        let mut takes = ConstantTake {
            operand,
            layout,
            count: 0,
        };
        takes.visit_fn(class);
        assert_eq!(takes.count, 1, "qualname must consume its actual operand");
        let catalog = late_bound_owner_field_site_catalog(&module, "pkg.mod");
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].1.owner_type.qualname, "Point");
        assert_eq!(catalog[0].1.storage, LateBoundOwnerFieldStorage::ObjectSlot);

        // Model only the new IR shape, not authenticated runtime authority.
        // Completing a source definition is an adoption boundary; the old
        // optional catalog must not infer a bare method binding through it.
        let method = module
            .callable_defs
            .iter()
            .find(|function| function.function_id == catalog[0].0)
            .unwrap();
        let method_source = "def read(self):\n        return self.value";
        let method_start = source
            .find(method_source)
            .expect("the original method source");
        let definition = SourceIdentity {
            module: ModuleContentId::new("pkg.mod", 0),
            lexical_qualname: method.names.qualname.clone(),
            source_range: soac_contracts::SourceRange::new(
                u32::try_from(method_start).unwrap(),
                u32::try_from(method_start + method_source.len()).unwrap(),
            ),
            definition_kind: DefinitionKind::Function,
        };
        let mut completed = module.clone();
        let binding = completed.callable_defs[class_index]
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.body)
            .find_map(|instr| {
                let InstrBlockPy::SetItem(store) = instr else {
                    return None;
                };
                matches!(store.replacement.as_ref(),
                    InstrBlockPy::MakeFunctionWithClosure(function)
                    if function.function_id() == method.function_id)
                .then_some(store)
            })
            .expect("the positive control has one original bare method binding");
        binding.replacement = Box::new(
            CompleteFunctionDefinition::new(
                definition,
                method.function_id,
                binding.replacement.clone(),
            )
            .into(),
        );
        assert!(
            late_bound_owner_field_site_catalog(&completed, "pkg.mod").is_empty(),
            "source definition completion alone does not select an optional late-owner plan",
        );

        let mut missing_layout = module.clone();
        missing_layout.callable_defs[class_index].storage_layout = None;
        assert!(
            late_bound_owner_field_site_catalog(&missing_layout, "pkg.mod").is_empty(),
            "a consuming read without a physical owner proof is not a constant binding",
        );

        let mut unmarked_operand = module.clone();
        unmarked_operand.callable_defs[class_index]
            .storage_layout
            .as_mut()
            .unwrap()
            .expression_temporaries
            .retain(|location| *location != OperandLocation::Local(operand));
        assert!(
            late_bound_owner_field_site_catalog(&unmarked_operand, "pkg.mod").is_empty(),
            "the same literal producer and spelling do not make an ordinary local an Operand",
        );

        let namespace_name = &class.params.params[class.params.positional_param_indices()[0]].name;
        let namespace = LocalLocation(
            u32::try_from(
                layout
                    .stack_slots
                    .iter()
                    .position(|name| name == namespace_name)
                    .unwrap(),
            )
            .unwrap(),
        );
        assert!(!layout.is_expression_temporary(namespace));
        struct RedirectTake {
            operand: LocalLocation,
            namespace: LocalLocation,
        }
        impl VisitMut<InstrBlockPy> for RedirectTake {
            fn visit_instr_mut(&mut self, expr: &mut InstrBlockPy) {
                if let InstrBlockPy::TakeOperand(take) = expr
                    && take.name.local_location() == Some(self.operand)
                {
                    // Leave its displayed name unchanged: only physical ownership counts.
                    take.name.location = NameLocation::Local(self.namespace);
                }
                expr.visit_children_mut(self);
            }
        }
        let mut wrong_owner = module.clone();
        RedirectTake { operand, namespace }
            .visit_fn_mut(&mut wrong_owner.callable_defs[class_index]);
        assert!(
            late_bound_owner_field_site_catalog(&wrong_owner, "pkg.mod").is_empty(),
            "a source namespace local cannot acquire Operand authority through its spelling",
        );

        let mut ambiguous = module.clone();
        ambiguous.callable_defs[class_index].blocks[producer_block]
            .body
            .push(InstrBlockPy::Store(producer.clone()));
        assert!(
            late_bound_owner_field_site_catalog(&ambiguous, "pkg.mod").is_empty(),
            "even equal literals from repeated stores must retain the existing ambiguity refusal",
        );
    }

    #[test]
    fn inherited_split_owner_catalog_reuses_one_anchor_per_concrete_owner_and_field() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class Root:
    def __init__(self, value):
        self.direction = True
        self.value = value

    def read(self):
        return self.value

    def read_again(self):
        return self.value

    def write(self, value):
        self.value = value

class Left(Root):
    pass

class Right(Root):
    def __init__(self, value):
        self.padding = 0
        Root.__init__(self, value)

class Grandchild(Right):
    pass

class SlottedRoot:
    __slots__ = ("value",)

    def read(self):
        return self.value

class SlottedChild(SlottedRoot):
    __slots__ = ()
"#,
        )
        .expect("literal inherited split-owner fixture should lower")
        .blockpy_module;

        let catalog = late_bound_owner_field_site_catalog(&lowered, "pkg.mod");
        let mut inherited_anchors = catalog
            .iter()
            .filter(|(_, plan)| {
                matches!(
                    plan.owner_type.qualname.as_str(),
                    "Left" | "Right" | "Grandchild"
                ) && matches!(plan.attr_name.as_str(), "value" | "direction")
            })
            .map(|(_, plan)| {
                (
                    plan.owner_type.qualname.as_str(),
                    plan.attr_name.as_str(),
                    plan.cell_index,
                )
            })
            .collect::<Vec<_>>();
        inherited_anchors.sort_unstable();
        let expected = [
            ("Grandchild", "direction"),
            ("Grandchild", "value"),
            ("Left", "direction"),
            ("Left", "value"),
            ("Right", "direction"),
            ("Right", "value"),
        ];
        assert_eq!(
            inherited_anchors
                .iter()
                .map(|(owner, attr, _)| (*owner, *attr))
                .collect::<Vec<_>>(),
            expected,
            "every literal same-module split descendant should share one stable owner/field anchor across inherited reads and writes"
        );
        assert!(
            catalog
                .iter()
                .enumerate()
                .all(|(index, (_, plan))| plan.cell_index == index as u32),
            "inherited anchors must preserve deterministic, dense catalog indices"
        );
        assert!(
            catalog
                .iter()
                .all(|(_, plan)| plan.owner_type.qualname != "SlottedChild"),
            "inherited object slots must remain outside split-owner specialization"
        );
    }

    #[test]
    fn inherited_split_owner_plans_cap_profiled_descendants_and_preserve_lexical_owner() {
        let mut source = String::from(
            "class Root:\n    def __init__(self, value):\n        self.value = value\n    def read(self):\n        return self.value\n\nclass AUnprofiled(Root):\n    pass\n",
        );
        for index in 0..10 {
            source.push_str(&format!("\nclass Child{index:02}(Root):\n    pass\n"));
        }
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(&source)
            .expect("bounded polymorphic owner fixture should lower")
            .blockpy_module;
        let reader = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "Root.read")
            .expect("bounded polymorphic fixture should contain Root.read");

        struct FieldSource(Option<InstrId>);

        impl Visit<InstrBlockPy> for FieldSource {
            fn visit_instr(&mut self, expr: &InstrBlockPy)
            where
                InstrBlockPy: ChildVisitable<InstrBlockPy>,
            {
                if let InstrBlockPy::GetAttr(getattr) = expr {
                    self.0 = Some(getattr.semantic_instr_id());
                }
                expr.visit_children(self);
            }
        }

        let mut field = FieldSource(None);
        field.visit_fn(reader);
        let field_source = field.0.expect("Root.read should access its receiver field");
        let mut field_row = row("field_access", reader.function_id, field_source, 64, None);
        field_row.branch_values = vec![soac_core::profile::CounterDumpBranchValue {
            branch: "generic_getattr".to_string(),
            value: 64,
        }];

        let mut owners = vec!["Root".to_string()];
        owners.extend((0..10).map(|index| format!("Child{index:02}")));
        let type_table = owners
            .iter()
            .enumerate()
            .map(|(index, owner)| CounterDumpTypeTableEntry {
                type_id: (index + 1) as u64,
                key: CounterDumpTypeKey {
                    module_name: "pkg.mod".to_string(),
                    qualname: owner.clone(),
                },
            })
            .collect::<Vec<_>>();
        let type_keys = owners
            .iter()
            .enumerate()
            .map(|(index, _)| CounterDumpTypeKeyLayout {
                owner_type_id: (index + 1) as u64,
                key: "value".to_string(),
                index: index as u32,
            })
            .collect::<Vec<_>>();
        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows: vec![field_row],
            module_keys: Vec::new(),
            type_keys,
            type_table,
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);

        let artifacts = plan_and_emit_module_v3_from_raw_evidence(
            &AlternativeCatalog::default_v3(),
            module_identity(),
            &lowered,
            &evidence_store,
        )
        .expect("profiled polymorphic owner fixture should plan and emit");
        let plans = &artifacts
            .plan
            .functions
            .iter()
            .find(|function| function.function.debug_name.as_deref() == Some("Root.read"))
            .expect("Root.read should receive a function plan")
            .late_bound_owner_fields;

        assert_eq!(
            plans.len(),
            8,
            "exact concrete owners must have a bounded guard chain"
        );
        assert_eq!(
            plans
                .iter()
                .map(|plan| (plan.owner_type.qualname.as_str(), plan.storage))
                .collect::<Vec<_>>(),
            vec![
                (
                    "Child00",
                    LateBoundOwnerFieldStorage::SplitDict { expected_index: 1 }
                ),
                (
                    "Child01",
                    LateBoundOwnerFieldStorage::SplitDict { expected_index: 2 }
                ),
                (
                    "Child02",
                    LateBoundOwnerFieldStorage::SplitDict { expected_index: 3 }
                ),
                (
                    "Child03",
                    LateBoundOwnerFieldStorage::SplitDict { expected_index: 4 }
                ),
                (
                    "Child04",
                    LateBoundOwnerFieldStorage::SplitDict { expected_index: 5 }
                ),
                (
                    "Child05",
                    LateBoundOwnerFieldStorage::SplitDict { expected_index: 6 }
                ),
                (
                    "Child06",
                    LateBoundOwnerFieldStorage::SplitDict { expected_index: 7 }
                ),
                (
                    "Root",
                    LateBoundOwnerFieldStorage::SplitDict { expected_index: 0 }
                ),
            ],
            "unprofiled classes must not consume the bound and an existing profiled lexical owner must never be displaced"
        );
        assert!(plans.iter().all(|plan| plan.source == field_source));
    }

    #[test]
    fn hot_nonself_uniform_split_fields_reuse_every_exact_owner_with_bounded_sources() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class Base:
    def __init__(self, value):
        self.link = value

    def read_self(self):
        return self.link

class Left(Base):
    pass

class Right(Base):
    pass

class Third(Base):
    pass

class Fourth(Base):
    pass

class Packet:
    def __init__(self, value):
        self.link = value

class MixedLeft:
    def __init__(self, value):
        self.mixed = value

class MixedRight:
    def __init__(self, value):
        self.padding = None
        self.mixed = value

class Unique:
    def __init__(self, value):
        self.payload = value

class LocalForeign:
    def __init__(self, value):
        self.external = value

class Anchored:
    def __init__(self, value):
        self.unanchored = value

class Unanchored:
    pass

class OverOne:
    def __init__(self, value):
        self.overflow = value

class OverTwo:
    def __init__(self, value):
        self.overflow = value

class OverThree:
    def __init__(self, value):
        self.overflow = value

class OverFour:
    def __init__(self, value):
        self.overflow = value

class OverFive:
    def __init__(self, value):
        self.overflow = value

class OverSix:
    def __init__(self, value):
        self.overflow = value

def read_uniform(owner):
    return owner.link

def write_uniform(owner, value):
    owner.link = value

def read_mixed(owner):
    return owner.mixed

def read_foreign(owner):
    return owner.external

def read_unanchored(owner):
    return owner.unanchored

def read_overflow(owner):
    return owner.overflow

def read_cold(owner):
    return owner.link

def read_unique(owner):
    return owner.payload

def write_unique(owner, value):
    owner.payload = value

def capped_uniform(owner):
    return (
        owner.link, owner.link, owner.link, owner.link, owner.link,
        owner.link, owner.link, owner.link, owner.link, owner.link,
    )
"#,
        )
        .expect("uniform polymorphic owner fixture should lower")
        .blockpy_module;

        struct FieldSources(Vec<(InstrId, IndexedFieldAccessKind)>);

        impl Visit<InstrBlockPy> for FieldSources {
            fn visit_instr(&mut self, expr: &InstrBlockPy)
            where
                InstrBlockPy: ChildVisitable<InstrBlockPy>,
            {
                match expr {
                    InstrBlockPy::GetAttr(op) => self
                        .0
                        .push((op.semantic_instr_id(), IndexedFieldAccessKind::Load)),
                    InstrBlockPy::SetAttr(op) => self
                        .0
                        .push((op.semantic_instr_id(), IndexedFieldAccessKind::Store)),
                    _ => {}
                }
                expr.visit_children(self);
            }
        }

        let mut rows = Vec::new();
        let mut fields_by_function = HashMap::new();
        let mut capped_counts = HashMap::new();
        for function in &lowered.callable_defs {
            let name = function.names.qualname.as_str();
            if !matches!(
                name,
                "Base.read_self"
                    | "read_uniform"
                    | "write_uniform"
                    | "read_mixed"
                    | "read_foreign"
                    | "read_unanchored"
                    | "read_overflow"
                    | "read_cold"
                    | "read_unique"
                    | "write_unique"
                    | "capped_uniform"
            ) {
                continue;
            }
            let mut sources = FieldSources(Vec::new());
            sources.visit_fn(function);
            for (index, (source, access)) in sources.0.iter().copied().enumerate() {
                let count = if name == "read_cold" {
                    7
                } else if name == "capped_uniform" {
                    32 + index as u64
                } else {
                    64
                };
                let mut field = row("field_access", function.function_id, source, count, None);
                field.branch_values = vec![soac_core::profile::CounterDumpBranchValue {
                    branch: match access {
                        IndexedFieldAccessKind::Load => "generic_getattr".to_string(),
                        IndexedFieldAccessKind::Store => "generic_setattr".to_string(),
                    },
                    value: count,
                }];
                rows.push(field);
                if name == "capped_uniform" {
                    capped_counts.insert(source, count);
                }
            }
            fields_by_function.insert(name.to_string(), sources.0);
        }

        let owners = [
            (1, "pkg.mod", "Left", "link", 0),
            (2, "pkg.mod", "Right", "link", 0),
            (3, "pkg.mod", "Third", "link", 0),
            (4, "pkg.mod", "Fourth", "link", 0),
            (5, "pkg.mod", "Packet", "link", 0),
            (6, "pkg.mod", "MixedLeft", "mixed", 0),
            (7, "pkg.mod", "MixedRight", "mixed", 1),
            (8, "pkg.mod", "Unique", "payload", 0),
            (9, "pkg.mod", "LocalForeign", "external", 0),
            (10, "pkg.foreign", "External", "external", 0),
            (11, "pkg.mod", "Anchored", "unanchored", 0),
            (12, "pkg.mod", "Unanchored", "unanchored", 0),
            (13, "pkg.mod", "OverOne", "overflow", 0),
            (14, "pkg.mod", "OverTwo", "overflow", 0),
            (15, "pkg.mod", "OverThree", "overflow", 0),
            (16, "pkg.mod", "OverFour", "overflow", 0),
            (17, "pkg.mod", "OverFive", "overflow", 0),
            (18, "pkg.mod", "OverSix", "overflow", 0),
        ];
        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows,
            module_keys: Vec::new(),
            type_table: owners
                .iter()
                .map(|(id, module_name, name, _, _)| CounterDumpTypeTableEntry {
                    type_id: *id,
                    key: CounterDumpTypeKey {
                        module_name: (*module_name).to_string(),
                        qualname: (*name).to_string(),
                    },
                })
                .collect(),
            type_keys: owners
                .iter()
                .map(|(id, _, _, key, index)| CounterDumpTypeKeyLayout {
                    owner_type_id: *id,
                    key: (*key).to_string(),
                    index: *index,
                })
                .collect(),
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);
        let artifacts = plan_and_emit_module_v3_from_raw_evidence(
            &AlternativeCatalog::default_v3(),
            module_identity(),
            &lowered,
            &evidence_store,
        )
        .expect("uniform polymorphic owner fixture should plan and emit");
        let planned = |name: &str| {
            artifacts
                .plan
                .functions
                .iter()
                .find(|function| function.function.debug_name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("missing function plan for {name}"))
        };

        for name in [
            "write_uniform",
            "read_foreign",
            "read_unanchored",
            "read_overflow",
            "read_cold",
        ] {
            assert!(
                planned(name).late_bound_owner_fields.is_empty(),
                "{name} must preserve its original generic operation"
            );
        }
        let mixed = &planned("read_mixed").late_bound_owner_fields;
        let mixed_owners = mixed
            .iter()
            .map(|plan| {
                assert_eq!(plan.source, fields_by_function["read_mixed"][0].0);
                assert_eq!(plan.access, IndexedFieldAccessKind::Load);
                assert_eq!(plan.attr_name, "mixed");
                let LateBoundOwnerFieldStorage::SplitDict { expected_index } = plan.storage else {
                    panic!("mixed non-self owners must retain their exact split-dict indices");
                };
                (plan.owner_type.qualname.as_str(), expected_index)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            mixed_owners,
            vec![("MixedLeft", 0), ("MixedRight", 1)],
            "both existing exact-owner guards must retain their independently profiled indices"
        );
        for name in ["read_unique", "write_unique"] {
            assert_eq!(
                planned(name).late_bound_owner_fields.len(),
                1,
                "existing unique-owner loads and stores must remain specialized"
            );
        }
        assert!(
            !planned("Base.read_self").late_bound_owner_fields.is_empty(),
            "existing inherited self-field guards must remain specialized"
        );

        let uniform = &planned("read_uniform").late_bound_owner_fields;
        assert_eq!(
            uniform.len(),
            5,
            "all four related same-index owners and the unrelated Packet owner must reuse exact existing cells"
        );
        let owners = uniform
            .iter()
            .map(|plan| plan.owner_type.qualname.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(
            owners,
            HashSet::from(["Left", "Right", "Third", "Fourth", "Packet"])
        );
        assert!(uniform.iter().all(|plan| {
            plan.source == fields_by_function["read_uniform"][0].0
                && plan.access == IndexedFieldAccessKind::Load
                && plan.storage == LateBoundOwnerFieldStorage::SplitDict { expected_index: 0 }
        }));

        let capped = &planned("capped_uniform").late_bound_owner_fields;
        let selected_sources = capped
            .iter()
            .map(|plan| plan.source)
            .collect::<HashSet<_>>();
        assert_eq!(selected_sources.len(), 8, "the bound counts source sites");
        assert_eq!(
            capped.len(),
            40,
            "every selected site retains all five owners"
        );
        let mut expected = capped_counts.into_iter().collect::<Vec<_>>();
        expected.sort_by_key(|(source, count)| (std::cmp::Reverse(*count), *source));
        expected.truncate(8);
        assert_eq!(
            selected_sources,
            expected
                .into_iter()
                .map(|(source, _)| source)
                .collect::<HashSet<_>>(),
            "the eight hottest distinct source sites retain complete owner groups"
        );
    }

    #[test]
    fn hot_nonself_split_fields_reuse_unique_owner_cells_with_bounded_sources() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class Record:
    def __init__(self, value):
        self.padding = None
        self.other = None
        self.payload = value
        self.external = value

    def read_self(self):
        return self.payload

class Left:
    def __init__(self, value):
        self.shared = value

class Right:
    def __init__(self, value):
        self.padding = None
        self.shared = value

class Slotted:
    __slots__ = ("slotted",)

    def __init__(self, value):
        self.slotted = value

def read_other(record):
    return record.payload

def write_other(record, value):
    record.payload = value

def cold_other(record):
    return record.payload

def ambiguous_other(record):
    return record.shared

def cross_module_other(record):
    return record.external

def slotted_other(record):
    return record.slotted

def unanchored_other(record):
    return record.missing

def capped_other(record):
    return (
        record.payload, record.payload, record.payload, record.payload,
        record.payload, record.payload, record.payload, record.payload,
        record.payload, record.payload,
    )

def scalar_other(record):
    if record.payload < 10:
        return 1
    return 0
"#,
        )
        .expect("non-self split-field fixture should lower")
        .blockpy_module;
        let owner_sites = late_bound_owner_field_site_catalog(&lowered, "pkg.mod");
        let initial_catalog_size = owner_sites.len();
        let anchor = owner_sites
            .iter()
            .map(|(_, site)| site)
            .filter(|site| {
                site.owner_type.qualname == "Record"
                    && site.attr_name == "payload"
                    && matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. })
            })
            .min_by_key(|site| site.cell_index)
            .expect("Record.payload should have an existing split-owner anchor");

        struct FieldSources<'a> {
            module: &'a BlockPyModule<BlockPyModuleShape>,
            fields: Vec<(InstrId, IndexedFieldAccessKind, String)>,
            comparisons: Vec<InstrId>,
        }

        impl Visit<InstrBlockPy> for FieldSources<'_> {
            fn visit_instr(&mut self, expr: &InstrBlockPy)
            where
                InstrBlockPy: ChildVisitable<InstrBlockPy>,
            {
                match expr {
                    InstrBlockPy::GetAttr(op) => {
                        if let Some(attr) =
                            codegen_constant_string_value_v3(self.module, op.attr.as_ref())
                        {
                            self.fields.push((
                                op.semantic_instr_id(),
                                IndexedFieldAccessKind::Load,
                                attr.to_string(),
                            ));
                        }
                    }
                    InstrBlockPy::SetAttr(op) => {
                        if let Some(attr) =
                            codegen_constant_string_value_v3(self.module, op.attr.as_ref())
                        {
                            self.fields.push((
                                op.semantic_instr_id(),
                                IndexedFieldAccessKind::Store,
                                attr.to_string(),
                            ));
                        }
                    }
                    InstrBlockPy::BinOp(op) if op.kind == BinOpKind::Lt => {
                        self.comparisons.push(op.semantic_instr_id());
                    }
                    _ => {}
                }
                expr.visit_children(self);
            }
        }

        let mut rows = Vec::new();
        let mut fields_by_function = HashMap::new();
        let mut source_counts = HashMap::new();
        for function in &lowered.callable_defs {
            let name = function.names.qualname.as_str();
            if !matches!(
                name,
                "Record.read_self"
                    | "read_other"
                    | "write_other"
                    | "cold_other"
                    | "ambiguous_other"
                    | "cross_module_other"
                    | "slotted_other"
                    | "unanchored_other"
                    | "capped_other"
                    | "scalar_other"
            ) {
                continue;
            }
            let mut sources = FieldSources {
                module: &lowered,
                fields: Vec::new(),
                comparisons: Vec::new(),
            };
            sources.visit_fn(function);
            for (index, (source, access, _)) in sources.fields.iter().enumerate() {
                let count = if name == "cold_other" {
                    7
                } else if name == "capped_other" {
                    20 + index as u64
                } else {
                    32
                };
                let mut field = row("field_access", function.function_id, *source, count, None);
                field.branch_values = vec![soac_core::profile::CounterDumpBranchValue {
                    branch: match access {
                        IndexedFieldAccessKind::Load => "generic_getattr".to_string(),
                        IndexedFieldAccessKind::Store => "generic_setattr".to_string(),
                    },
                    value: count,
                }];
                rows.push(field);
                if name == "capped_other" {
                    source_counts.insert(*source, count);
                }
            }
            for source in sources.comparisons {
                rows.push(row(
                    "operator_hot_shapes",
                    function.function_id,
                    source,
                    32,
                    Some(pack_binary_shape(ExactTypeTag::Int, ExactTypeTag::Int)),
                ));
            }
            fields_by_function.insert(name.to_string(), sources.fields);
        }

        let owners = [
            (44, "pkg.mod", "Record"),
            (45, "pkg.mod", "Left"),
            (46, "pkg.mod", "Right"),
            (47, "pkg.mod", "Slotted"),
            (48, "pkg.mod", "Missing"),
            (49, "pkg.foreign", "External"),
        ];
        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows,
            module_keys: Vec::new(),
            type_table: owners
                .iter()
                .map(|(id, module_name, name)| CounterDumpTypeTableEntry {
                    type_id: *id,
                    key: CounterDumpTypeKey {
                        module_name: (*module_name).to_string(),
                        qualname: (*name).to_string(),
                    },
                })
                .collect(),
            type_keys: vec![
                CounterDumpTypeKeyLayout {
                    owner_type_id: 44,
                    key: "payload".to_string(),
                    index: 2,
                },
                CounterDumpTypeKeyLayout {
                    owner_type_id: 45,
                    key: "shared".to_string(),
                    index: 0,
                },
                CounterDumpTypeKeyLayout {
                    owner_type_id: 46,
                    key: "shared".to_string(),
                    index: 1,
                },
                CounterDumpTypeKeyLayout {
                    owner_type_id: 47,
                    key: "slotted".to_string(),
                    index: 0,
                },
                CounterDumpTypeKeyLayout {
                    owner_type_id: 48,
                    key: "missing".to_string(),
                    index: 0,
                },
                CounterDumpTypeKeyLayout {
                    owner_type_id: 44,
                    key: "external".to_string(),
                    index: 3,
                },
                CounterDumpTypeKeyLayout {
                    owner_type_id: 49,
                    key: "external".to_string(),
                    index: 0,
                },
            ],
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);

        let artifacts = plan_and_emit_module_v3_from_raw_evidence(
            &AlternativeCatalog::default_v3(),
            module_identity(),
            &lowered,
            &evidence_store,
        )
        .expect("hot non-self split-field fixture should plan and emit");
        let planned = |name: &str| {
            artifacts
                .plan
                .functions
                .iter()
                .find(|function| function.function.debug_name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("missing function plan for {name}"))
        };

        for (name, access) in [
            ("read_other", IndexedFieldAccessKind::Load),
            ("write_other", IndexedFieldAccessKind::Store),
        ] {
            let source = fields_by_function[name][0].0;
            let selected = planned(name)
                .late_bound_owner_fields
                .iter()
                .find(|plan| plan.source == source)
                .unwrap_or_else(|| {
                    panic!(
                        "hot non-self {access:?} at {source} should reuse existing Record.payload cell"
                    )
                });
            assert_eq!(selected.access, access);
            assert_eq!(selected.owner_type, anchor.owner_type);
            assert_eq!(selected.attr_name, "payload");
            assert_eq!(selected.cell_index, anchor.cell_index);
            assert_eq!(
                selected.storage,
                LateBoundOwnerFieldStorage::SplitDict { expected_index: 2 },
            );
        }

        for name in [
            "cold_other",
            "cross_module_other",
            "slotted_other",
            "unanchored_other",
        ] {
            assert!(
                planned(name).late_bound_owner_fields.is_empty(),
                "{name} must retain generic attribute access"
            );
        }
        let mixed = &planned("ambiguous_other").late_bound_owner_fields;
        let mixed_owners = mixed
            .iter()
            .map(|plan| {
                assert_eq!(plan.source, fields_by_function["ambiguous_other"][0].0);
                assert_eq!(plan.access, IndexedFieldAccessKind::Load);
                assert_eq!(plan.attr_name, "shared");
                let LateBoundOwnerFieldStorage::SplitDict { expected_index } = plan.storage else {
                    panic!("mixed non-self owners must retain their exact split-dict indices");
                };
                (plan.owner_type.qualname.as_str(), expected_index)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            mixed_owners,
            vec![("Left", 0), ("Right", 1)],
            "both profiled non-self owners must reuse their existing guard cells and indices"
        );

        let scalar = &planned("scalar_other").late_bound_owner_fields;
        assert_eq!(scalar.len(), 1);
        assert!(scalar[0].reason.contains("scalar region"));
        assert!(
            planned("Record.read_self")
                .late_bound_owner_fields
                .iter()
                .any(|plan| plan.attr_name == "payload"),
            "preexisting lexical-owner selection must remain intact"
        );

        let capped = &planned("capped_other").late_bound_owner_fields;
        assert_eq!(capped.len(), 8);
        let mut expected = fields_by_function["capped_other"]
            .iter()
            .map(|(source, access, _)| (*source, *access))
            .collect::<Vec<_>>();
        expected.sort_by_key(|(source, access)| {
            (std::cmp::Reverse(source_counts[source]), *source, *access)
        });
        expected.truncate(8);
        assert_eq!(
            capped
                .iter()
                .map(|plan| (plan.source, plan.access))
                .collect::<Vec<_>>(),
            expected,
            "only the deterministically ordered eight hottest non-self sites should be selected"
        );
        assert_eq!(
            late_bound_owner_field_site_catalog(&lowered, "pkg.mod").len(),
            initial_catalog_size,
            "general non-self selection must reuse existing owner cells"
        );
    }

    #[test]
    fn late_bound_scalar_regions_reuse_existing_split_owner_constructor_cells() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class Record:
    def __init__(self, value):
        self.first = 0
        self.second = 0
        self.value = value

def branch(record):
    if record.value < 10:
        return 1
    return 0

class Handler:
    def branch(self, record):
        if record.value < 10:
            return 1
        return 0
"#,
        )
        .expect("non-self scalar owner fixture should lower")
        .blockpy_module;
        let owner_sites = late_bound_owner_field_site_catalog(&lowered, "pkg.mod");
        let anchor = owner_sites
            .iter()
            .find(|(_, site)| {
                site.owner_type.qualname == "Record"
                    && site.attr_name == "value"
                    && site.access == IndexedFieldAccessKind::Store
                    && matches!(site.storage, LateBoundOwnerFieldStorage::SplitDict { .. })
            })
            .map(|(_, site)| site)
            .expect("the Record constructor should already own a reusable split-field cell");

        #[derive(Default)]
        struct ConsumerSites {
            field: Option<InstrId>,
            comparison: Option<InstrId>,
        }

        impl Visit<InstrBlockPy> for ConsumerSites {
            fn visit_instr(&mut self, expr: &InstrBlockPy)
            where
                InstrBlockPy: ChildVisitable<InstrBlockPy>,
            {
                match expr {
                    InstrBlockPy::GetAttr(op) => self.field = Some(op.semantic_instr_id()),
                    InstrBlockPy::BinOp(op) if op.kind == BinOpKind::Lt => {
                        self.comparison = Some(op.semantic_instr_id());
                    }
                    _ => {}
                }
                expr.visit_children(self);
            }
        }

        let consumers = lowered
            .callable_defs
            .iter()
            .filter(|function| {
                matches!(
                    function.names.qualname.as_str(),
                    "branch" | "Handler.branch"
                )
            })
            .map(|function| {
                let mut sites = ConsumerSites::default();
                sites.visit_fn(function);
                (
                    function,
                    sites.field.expect("consumer should contain a field load"),
                    sites
                        .comparison
                        .expect("consumer should contain an exact-int comparison"),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(consumers.len(), 2);

        let mut rows = Vec::new();
        for (function, field_source, comparison_source) in &consumers {
            let mut field = row(
                "field_access",
                function.function_id,
                *field_source,
                16,
                None,
            );
            field.branch_values = vec![soac_core::profile::CounterDumpBranchValue {
                branch: "generic_getattr".to_string(),
                value: 16,
            }];
            rows.push(field);
            rows.push(row(
                "operator_hot_shapes",
                function.function_id,
                *comparison_source,
                16,
                Some(pack_binary_shape(ExactTypeTag::Int, ExactTypeTag::Int)),
            ));
        }
        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows,
            module_keys: Vec::new(),
            type_keys: vec![CounterDumpTypeKeyLayout {
                owner_type_id: 44,
                key: "value".to_string(),
                index: 2,
            }],
            type_table: vec![CounterDumpTypeTableEntry {
                type_id: 44,
                key: CounterDumpTypeKey {
                    module_name: "pkg.mod".to_string(),
                    qualname: "Record".to_string(),
                },
            }],
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);

        let artifacts = plan_and_emit_module_v3_from_raw_evidence(
            &AlternativeCatalog::default_v3(),
            module_identity(),
            &lowered,
            &evidence_store,
        )
        .expect("the profiled non-self scalar fixture should plan and emit");

        for (consumer, field_source, _) in consumers {
            let function = artifacts
                .plan
                .functions
                .iter()
                .find(|function| {
                    function.function.debug_name.as_deref()
                        == Some(consumer.names.qualname.as_str())
                })
                .expect("consumer should have a v3 function plan");
            assert!(
                function.regions.iter().any(|region| {
                    region.inputs.iter().any(|input| {
                        matches!(
                            &input.source,
                            soac_ir_typed::plan_v3::RegionInputSource::IndexedField {
                                source,
                                owner_type,
                                attr_name,
                                expected_index,
                                ..
                            } if *source == field_source
                                && owner_type.module_name == "pkg.mod"
                                && owner_type.qualname == "Record"
                                && attr_name == "value"
                                && *expected_index == 2
                                && input.value.rep == soac_ir_typed::plan_v3::Rep::PyObjectBorrowed
                        )
                    })
                }),
                "{} should have an actual profiled borrowed scalar field input: {:?}",
                consumer.names.qualname,
                function.regions,
            );
            let late_plan = function
                .late_bound_owner_fields
                .iter()
                .find(|plan| plan.source == field_source)
                .unwrap_or_else(|| {
                    panic!(
                        "{} must reuse Record.value's existing constructor owner cell for its non-self scalar input; selected plans: {:?}",
                        consumer.names.qualname, function.late_bound_owner_fields,
                    )
                });
            assert_eq!(late_plan.owner_type, anchor.owner_type);
            assert_eq!(late_plan.attr_name, anchor.attr_name);
            assert_eq!(late_plan.cell_index, anchor.cell_index);
            assert_eq!(late_plan.access, IndexedFieldAccessKind::Load);
            assert_eq!(
                late_plan.storage,
                LateBoundOwnerFieldStorage::SplitDict { expected_index: 2 },
            );
        }
    }

    use crate::operator_specialization::{ExactTypeTag, pack_binary_shape};
    use crate::region_v3::{ExtractedValueId, extract_block_region_v3};
    use soac_core::block_py::literal::{LiteralValue, StringLiteral};
    use soac_core::block_py::{
        BinOp, BinOpKind, Block, BlockLabel, BlockParam, BlockPyName, BlockTerm, Call,
        CallArgPositional, FunctionName, GetAttr, GetItem, InstrId, Load, LocalFunctionId,
        LocalLocation, Meta, ModuleNameGen, NameLocation, Param, ParamSpec, ResolvedName,
        RuntimeFunctionId, SerializedFunctionId, SerializedModuleId, SetAttr, SetItem,
        StorageLayout, Store, TermIf, VisitMut, WithMeta,
    };
    use soac_core::profile::{
        CounterDumpKeyLayout, CounterDumpRecord, CounterDumpRow, CounterDumpTypeKey,
        CounterDumpTypeKeyLayout, CounterDumpTypeTableEntry,
    };
    use soac_ir_blockpy::{
        CONSTRUCTOR_ENTRY_FUNCTION_NAME, CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME, InstrBlockPy,
    };
    use soac_ir_typed::plan_v3::{CallBodyKind, RegionId, validate_module_plan_v3};
    use soac_ir_typed::{
        InstrTyped, TypedExactFloatExpressionPlan, lower_blockpy_function_to_typed,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn label(index: usize) -> BlockLabel {
        BlockLabel::from_index(index)
    }

    fn instr_id(index: u32) -> InstrId {
        InstrId::new(index)
    }

    fn with_instr_id(instr: InstrBlockPy, index: u32) -> InstrBlockPy {
        instr.with_meta(Meta {
            instr_id: Some(instr_id(index)),
            ..Meta::synthetic()
        })
    }

    fn local_name(name: &str, slot: u32) -> ResolvedName {
        ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Local(LocalLocation(slot)),
        }
    }

    fn local(name: &str, slot: u32) -> InstrBlockPy {
        InstrBlockPy::Load(Load::new(local_name(name, slot)))
    }

    fn constant_name(index: u32) -> InstrBlockPy {
        InstrBlockPy::Load(Load::new(ResolvedName {
            id: BlockPyName::new(format!("<const {index}>")),
            location: NameLocation::Constant(index),
        }))
    }

    fn runtime_name(name: RuntimeName) -> InstrBlockPy {
        InstrBlockPy::Load(Load::new(ResolvedName {
            id: BlockPyName::new(name.name()),
            location: NameLocation::RuntimeName(name),
        }))
    }

    fn global_name(name: &str, slot: u32) -> ResolvedName {
        ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Global(soac_core::block_py::GlobalSlot(slot)),
        }
    }

    fn binary(op: BinOpKind, left: InstrBlockPy, right: InstrBlockPy, id: u32) -> InstrBlockPy {
        with_instr_id(InstrBlockPy::BinOp(BinOp::new(op, left, right)), id)
    }

    fn branch_block() -> Block<InstrBlockPy> {
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
        blocks: Vec<Block<InstrBlockPy>>,
    ) -> BlockPyFunction<BlockPyModuleShape> {
        let name_gen = ModuleNameGen::new(0).next_function_name_gen();
        function_with_name_gen(name_gen, "f", "f", blocks)
    }

    fn function_with_name_gen(
        name_gen: soac_core::block_py::FunctionNameGen,
        fn_name: &str,
        qualname: &str,
        blocks: Vec<Block<InstrBlockPy>>,
    ) -> BlockPyFunction<BlockPyModuleShape> {
        BlockPyFunction {
            function_id: name_gen.function_id(),
            name_gen,
            names: FunctionName::new(fn_name, fn_name, qualname, qualname),
            kind: soac_core::block_py::FunctionKind::Function,
            execution_mode: Default::default(),
            params: ParamSpec::default(),
            body_params: None,
            public_scope: None,
            blocks,
            doc: None,
            public_storage_layout: None,
            storage_layout: None,
            scope: Default::default(),
        }
    }

    fn module_with_constants(
        module_constants: Vec<ConstantExpr>,
    ) -> BlockPyModule<BlockPyModuleShape> {
        BlockPyModule {
            strict_source: None,
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: Vec::new(),
            module_constants,
            counter_defs: Vec::new(),
        }
    }

    fn set_stack_slots(function: &mut BlockPyFunction<BlockPyModuleShape>, names: &[&str]) {
        function
            .storage_layout
            .get_or_insert_with(StorageLayout::default)
            .set_stack_slots(names.iter().map(|name| (*name).to_string()).collect());
    }

    fn any_param(name: &str, has_default: bool) -> Param {
        Param {
            name: name.to_string(),
            kind: ParamKind::Any,
            has_default,
        }
    }

    fn simple_arg_return_callee(params: &[(&str, bool)]) -> BlockPyFunction<BlockPyModuleShape> {
        let mut function = function_with_blocks(vec![Block::new(
            label(0),
            Vec::new(),
            BlockTerm::Return(local(params[0].0, 0)),
            Vec::<BlockParam>::new(),
            None,
        )]);
        function.params.params = params
            .iter()
            .map(|(name, has_default)| any_param(name, *has_default))
            .collect();
        let slots = params.iter().map(|(name, _)| *name).collect::<Vec<_>>();
        set_stack_slots(&mut function, slots.as_slice());
        function
    }

    fn direct_call_target_entry(
        module: ModuleContentId,
        function: BlockPyFunction<BlockPyModuleShape>,
    ) -> DirectCallTargetEntry {
        DirectCallTargetEntry {
            module,
            function,
            module_constants: Vec::new(),
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

    fn row(
        kind: &str,
        function_id: RuntimeFunctionId,
        instr_id: InstrId,
        value: u64,
        observed_value: Option<u64>,
    ) -> CounterDumpRow {
        CounterDumpRow {
            counter_id: 0,
            scope: "function".to_string(),
            kind: kind.to_string(),
            site_kind: kind.to_string(),
            function_id: Some(function_id),
            current_function_id: Some(function_id),
            instr_id: Some(instr_id),
            function_qualname: Some("f".to_string()),
            block_label: Some("bb0".to_string()),
            value,
            branch_values: Vec::new(),
            observed_value,
            max_overcount: None,
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
    fn direct_call_inline_candidate_requires_buildable_inline_body() {
        let source = instr_id(9);
        let current_module = ModuleContentId::new("pkg.mod", 0x99);
        let call = with_instr_id(
            InstrBlockPy::Call(Call::new(
                local("callee", 0),
                vec![CallArgPositional::Positional(local("x", 1))],
                Vec::new(),
            )),
            9,
        );
        let mut caller = function_with_blocks(vec![Block::new(
            label(0),
            vec![InstrBlockPy::Store(Store::new(
                local_name("result", 2),
                call,
            ))],
            BlockTerm::Return(local("result", 2)),
            Vec::<BlockParam>::new(),
            None,
        )]);
        set_stack_slots(&mut caller, &["callee", "x", "result"]);
        let module = module_with_constants(Vec::new());

        let exact_callee = simple_arg_return_callee(&[("x", false)]);
        let exact_plan = direct_call_arg_plan_for_instr_id_v3(&caller, source, &exact_callee, 0)
            .unwrap()
            .unwrap();
        assert_eq!(exact_plan.sources, vec![DirectCallArgSource::Provided(0)]);
        let exact_target = direct_call_target_entry(current_module.clone(), exact_callee);
        assert!(direct_call_inline_candidate_v3(
            &module,
            &current_module,
            &caller,
            source,
            &exact_target,
            &DirectCallCallee::Function,
            &exact_plan,
        ));
        let cross_module_target = direct_call_target_entry(
            ModuleContentId::new("pkg.external", 0x100),
            simple_arg_return_callee(&[("x", false)]),
        );
        assert!(
            direct_call_inline_candidate_v3(
                &module,
                &current_module,
                &caller,
                source,
                &cross_module_target,
                &DirectCallCallee::Function,
                &exact_plan,
            ),
            "cross-module callees should be inline candidates when the body is remappable"
        );

        let effect_source = instr_id(10);
        let mut effect_only_caller = function_with_blocks(vec![Block::new(
            label(0),
            vec![with_instr_id(
                InstrBlockPy::Call(Call::new(
                    local("callee", 0),
                    vec![CallArgPositional::Positional(local("x", 1))],
                    Vec::new(),
                )),
                10,
            )],
            BlockTerm::Return(local("x", 1)),
            Vec::<BlockParam>::new(),
            None,
        )]);
        set_stack_slots(&mut effect_only_caller, &["callee", "x"]);
        assert!(direct_call_inline_candidate_v3(
            &module,
            &current_module,
            &effect_only_caller,
            effect_source,
            &exact_target,
            &DirectCallCallee::Function,
            &exact_plan,
        ));

        let return_source = instr_id(11);
        let mut return_caller = function_with_blocks(vec![Block::new(
            label(0),
            Vec::new(),
            BlockTerm::Return(with_instr_id(
                InstrBlockPy::Call(Call::new(
                    local("callee", 0),
                    vec![CallArgPositional::Positional(local("x", 1))],
                    Vec::new(),
                )),
                11,
            )),
            Vec::<BlockParam>::new(),
            None,
        )]);
        set_stack_slots(&mut return_caller, &["callee", "x"]);
        assert!(direct_call_inline_candidate_v3(
            &module,
            &current_module,
            &return_caller,
            return_source,
            &exact_target,
            &DirectCallCallee::Function,
            &exact_plan,
        ));

        let constructor_source = instr_id(12);
        let mut constructor_caller = function_with_blocks(vec![Block::new(
            label(0),
            vec![InstrBlockPy::Store(Store::new(
                local_name("result", 2),
                with_instr_id(
                    InstrBlockPy::Call(Call::new(
                        local("Record", 0),
                        vec![CallArgPositional::Positional(local("x", 1))],
                        Vec::new(),
                    )),
                    12,
                ),
            ))],
            BlockTerm::Return(local("result", 2)),
            Vec::<BlockParam>::new(),
            None,
        )]);
        set_stack_slots(&mut constructor_caller, &["Record", "x", "result"]);
        let mut constructor_entry =
            simple_arg_return_callee(&[(CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME, false), ("x", false)]);
        constructor_entry.names = FunctionName::new(
            CONSTRUCTOR_ENTRY_FUNCTION_NAME,
            CONSTRUCTOR_ENTRY_FUNCTION_NAME,
            "Record.__soac_constructor_entry__#0:1",
            "Record.__soac_constructor_entry__#0:1",
        );
        let constructor_plan = direct_call_arg_plan_for_instr_id_v3(
            &constructor_caller,
            constructor_source,
            &constructor_entry,
            1,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            constructor_plan.sources,
            vec![
                DirectCallArgSource::Provided(0),
                DirectCallArgSource::Provided(1),
            ]
        );
        let constructor_target =
            direct_call_target_entry(current_module.clone(), constructor_entry);
        assert!(direct_call_inline_candidate_v3(
            &module,
            &current_module,
            &constructor_caller,
            constructor_source,
            &constructor_target,
            &DirectCallCallee::Function,
            &constructor_plan,
        ));

        let mut vararg_constructor_entry = function_with_blocks(vec![Block::new(
            label(0),
            Vec::new(),
            BlockTerm::Return(InstrBlockPy::Call(Call::new(
                runtime_name(RuntimeName::ConstructorCall),
                vec![
                    CallArgPositional::Positional(local(CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME, 0)),
                    CallArgPositional::Starred(local("args", 1)),
                ],
                Vec::new(),
            ))),
            Vec::<BlockParam>::new(),
            None,
        )]);
        vararg_constructor_entry.names = FunctionName::new(
            CONSTRUCTOR_ENTRY_FUNCTION_NAME,
            CONSTRUCTOR_ENTRY_FUNCTION_NAME,
            "Record.__soac_constructor_entry__#0:2",
            "Record.__soac_constructor_entry__#0:2",
        );
        vararg_constructor_entry.params.params = vec![
            any_param(CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME, false),
            Param {
                name: "args".to_string(),
                kind: ParamKind::VarArg,
                has_default: false,
            },
        ];
        set_stack_slots(
            &mut vararg_constructor_entry,
            &[CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME, "args"],
        );
        let vararg_constructor_plan = direct_call_arg_plan_for_instr_id_v3(
            &constructor_caller,
            constructor_source,
            &vararg_constructor_entry,
            1,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            vararg_constructor_plan.sources,
            vec![
                DirectCallArgSource::Provided(0),
                DirectCallArgSource::PackedRest { start: 1 },
            ]
        );
        let vararg_constructor_target =
            direct_call_target_entry(current_module.clone(), vararg_constructor_entry);
        assert!(direct_call_inline_candidate_v3(
            &module,
            &current_module,
            &constructor_caller,
            constructor_source,
            &vararg_constructor_target,
            &DirectCallCallee::Function,
            &vararg_constructor_plan,
        ));

        let default_callee = simple_arg_return_callee(&[("x", false), ("y", true)]);
        let default_plan =
            direct_call_arg_plan_for_instr_id_v3(&caller, source, &default_callee, 0)
                .unwrap()
                .unwrap();
        assert_eq!(
            default_plan.sources,
            vec![
                DirectCallArgSource::Provided(0),
                DirectCallArgSource::DefaultSentinel
            ]
        );
        let default_target = direct_call_target_entry(current_module.clone(), default_callee);
        assert!(!direct_call_inline_candidate_v3(
            &module,
            &current_module,
            &caller,
            source,
            &default_target,
            &DirectCallCallee::Function,
            &default_plan,
        ));
    }

    #[test]
    fn direct_call_requests_preserve_exact_stop_iteration_runtime_fast_path() {
        let module_name_gen = ModuleNameGen::new(0);
        let make_call = |source, callable, handler| {
            InstrBlockPy::Store(Store::new(
                local_name("result", 3),
                with_instr_id(
                    InstrBlockPy::Call(Call::new(
                        callable,
                        vec![
                            CallArgPositional::Positional(local("error", 0)),
                            CallArgPositional::Positional(handler),
                        ],
                        Vec::new(),
                    )),
                    source,
                ),
            ))
        };
        let mut caller = function_with_name_gen(
            module_name_gen.next_function_name_gen(),
            "caller",
            "caller",
            vec![Block::new(
                label(0),
                vec![
                    make_call(
                        10,
                        runtime_name(RuntimeName::ExceptionMatches),
                        runtime_name(RuntimeName::StopIteration),
                    ),
                    make_call(11, constant_name(0), constant_name(1)),
                    make_call(
                        12,
                        runtime_name(RuntimeName::ExceptionMatches),
                        runtime_name(RuntimeName::ValueError),
                    ),
                    make_call(
                        13,
                        runtime_name(RuntimeName::ExceptionMatches),
                        local("handler", 1),
                    ),
                    make_call(
                        14,
                        local("callback", 2),
                        runtime_name(RuntimeName::StopIteration),
                    ),
                ],
                BlockTerm::Return(local("result", 3)),
                Vec::<BlockParam>::new(),
                None,
            )],
        );
        set_stack_slots(&mut caller, &["error", "handler", "callback", "result"]);

        let mut target = function_with_name_gen(
            module_name_gen.next_function_name_gen(),
            "exception_matches",
            "exception_matches",
            vec![Block::new(
                label(0),
                Vec::new(),
                BlockTerm::Return(local("error", 0)),
                Vec::<BlockParam>::new(),
                None,
            )],
        );
        target.params.params = vec![any_param("error", false), any_param("expected", false)];
        set_stack_slots(&mut target, &["error", "expected"]);

        let caller_id = caller.function_id;
        let target_id = target.function_id;
        let module = BlockPyModule {
            strict_source: None,
            module_name_gen,
            global_names: Vec::new(),
            callable_defs: vec![caller.clone(), target],
            module_constants: vec![
                ConstantExpr::RuntimeName(RuntimeName::ExceptionMatches),
                ConstantExpr::RuntimeName(RuntimeName::StopIteration),
            ],
            counter_defs: Vec::new(),
        };
        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows: (10..=14)
                .map(|source| {
                    row(
                        "call_hot_targets",
                        caller_id,
                        instr_id(source),
                        32,
                        Some(target_id.to_packed_runtime_u64()),
                    )
                })
                .collect(),
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);
        let module_identity = module_identity();
        let target_index = DirectCallTargetIndex::from_current_module(&module_identity, &module);
        let mut identity_builder = OptimizationPlanV3IdentityBuilder::new(&module_identity);

        let (requests, diagnostics) = direct_call_requests_from_evidence_v3(
            &module,
            &module_identity,
            &caller,
            &evidence_store,
            &target_index,
            &mut identity_builder,
        );

        assert_eq!(
            requests
                .iter()
                .map(|request| request.source)
                .collect::<Vec<_>>(),
            vec![instr_id(12), instr_id(13), instr_id(14)],
            "only proven compiler-owned StopIteration matches should retain the guarded generic vectorcall path"
        );
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.source)
                .collect::<Vec<_>>(),
            vec![Some(instr_id(10)), Some(instr_id(11))],
            "both explicit runtime names and their module-constant aliases need the same planning decision"
        );
    }

    #[test]
    fn direct_call_plans_preserve_eager_comprehension_callable_elision() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def ordinary(value):
    return value

def caller(offset, values):
    items = [(offset, value) for value in values]
    unique = {(offset, value) for value in values}
    indexed = {value: (offset, value) for value in values}
    lazy = (value for value in values)

    def _dp_listcomp_777(value):
        return value

    return items, unique, indexed, lazy, ordinary(offset), _dp_listcomp_777(offset)
"#,
        )
        .expect("eager comprehension direct-call fixture should lower")
        .blockpy_module;
        let caller = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("fixture should retain its original caller");

        #[derive(Default)]
        struct Calls {
            sources: Vec<(InstrId, String)>,
        }

        impl Visit<InstrBlockPy> for Calls {
            fn visit_instr(&mut self, expr: &InstrBlockPy)
            where
                InstrBlockPy: ChildVisitable<InstrBlockPy>,
            {
                if let InstrBlockPy::Call(call) = expr
                    && let InstrBlockPy::Load(callee) = call.func.as_ref()
                {
                    self.sources
                        .push((call.semantic_instr_id(), callee.name.id_str().to_string()));
                }
                expr.visit_children(self);
            }
        }

        let mut calls = Calls::default();
        calls.visit_fn(caller);
        let mut rows = lowered
            .callable_defs
            .iter()
            .map(|function| row("function_entry", function.function_id, instr_id(0), 1, None))
            .collect::<Vec<_>>();
        let mut generated_sources = Vec::new();
        let mut ordinary_source = None;
        let mut spoof_source = None;
        let mut lazy_source = None;

        for (source, bind_name) in calls.sources {
            let Some(target) = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == bind_name)
            else {
                continue;
            };
            rows.push(row(
                "call_hot_targets",
                caller.function_id,
                source,
                32,
                Some(target.function_id.to_packed_runtime_u64()),
            ));

            match target.names.display_name.as_str() {
                "<listcomp>" | "<setcomp>" | "<dictcomp>"
                    if target.params.params.len() == 1
                        && target.params.params[0].name.starts_with("_dp_iter_") =>
                {
                    generated_sources.push(source);
                }
                "<listcomp>" if bind_name == "_dp_listcomp_777" => {
                    spoof_source = Some(source);
                }
                "<genexpr>" => lazy_source = Some(source),
                "ordinary" => ordinary_source = Some(source),
                _ => {}
            }
        }

        assert_eq!(
            generated_sources.len(),
            3,
            "all three eager display kinds must be profiled"
        );
        let ordinary_source = ordinary_source.expect("ordinary source function must be profiled");
        let spoof_source =
            spoof_source.expect("source-authored generated-name spoof must be profiled");
        assert!(
            lazy_source.is_some(),
            "real generator factory must remain represented"
        );

        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);

        let artifacts = plan_and_emit_module_v3_from_raw_evidence(
            &AlternativeCatalog::default_v3(),
            module_identity(),
            &lowered,
            &evidence_store,
        )
        .expect("actual module optimization pipeline should plan profiled callable targets");
        let caller_plan = artifacts
            .plan
            .functions
            .iter()
            .find(|function| function.function.debug_name.as_deref() == Some("caller"))
            .expect("original caller should receive its production optimization plan");
        let selected_sources = caller_plan
            .direct_calls
            .iter()
            .map(|plan| plan.source)
            .collect::<Vec<_>>();

        assert!(
            selected_sources.contains(&ordinary_source),
            "ordinary direct targets must remain eligible: {selected_sources:?}"
        );
        assert!(
            selected_sources.contains(&spoof_source),
            "source-authored generated-name spoofs must remain ordinary Python function targets: {selected_sources:?}"
        );
        assert!(
            generated_sources
                .iter()
                .all(|source| !selected_sources.contains(source)),
            "compiler-generated eager comprehension calls must use their callable generic path without stale exact-PyFunction guards: generated={generated_sources:?}, selected={selected_sources:?}"
        );
    }

    #[test]
    fn direct_call_requests_decline_legacy_constructor_targets_without_entry_thunk() {
        let module_name_gen = ModuleNameGen::new(0);
        let source = instr_id(9);
        let call = with_instr_id(
            InstrBlockPy::Call(Call::new(
                local("Record", 0),
                vec![CallArgPositional::Positional(local("value", 1))],
                Vec::new(),
            )),
            9,
        );
        let mut caller = function_with_name_gen(
            module_name_gen.next_function_name_gen(),
            "caller",
            "caller",
            vec![Block::new(
                label(0),
                vec![InstrBlockPy::Store(Store::new(
                    local_name("result", 2),
                    call,
                ))],
                BlockTerm::Return(local("result", 2)),
                Vec::<BlockParam>::new(),
                None,
            )],
        );
        set_stack_slots(&mut caller, &["Record", "value", "result"]);
        let mut init = function_with_name_gen(
            module_name_gen.next_function_name_gen(),
            "__init__",
            "Record.__init__",
            vec![Block::new(
                label(0),
                Vec::new(),
                BlockTerm::Return(local("self", 0)),
                Vec::<BlockParam>::new(),
                None,
            )],
        );
        init.params.params = vec![
            any_param("self", false),
            any_param("value", false),
            any_param("defaulted", true),
        ];
        set_stack_slots(&mut init, &["self", "value", "defaulted"]);
        let caller_id = caller.function_id;
        let init_id = init.function_id;
        let module = BlockPyModule {
            strict_source: None,
            module_name_gen,
            global_names: Vec::new(),
            callable_defs: vec![caller.clone(), init],
            module_constants: Vec::new(),
            counter_defs: Vec::new(),
        };
        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows: vec![row(
                "call_hot_targets",
                caller_id,
                source,
                1,
                Some(init_id.to_packed_runtime_u64()),
            )],
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);
        let module_identity = module_identity();
        let target_index = DirectCallTargetIndex::from_current_module(&module_identity, &module);
        let mut identity_builder = OptimizationPlanV3IdentityBuilder::new(&module_identity);

        let (requests, diagnostics) = direct_call_requests_from_evidence_v3(
            &module,
            &module_identity,
            &caller,
            &evidence_store,
            &target_index,
            &mut identity_builder,
        );

        assert!(requests.is_empty(), "{requests:?}");
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .message
                .contains("missing required argument value")
        );
    }

    #[test]
    fn direct_call_requests_decline_generator_like_targets() {
        let module_name_gen = ModuleNameGen::new(0);
        let source = instr_id(9);
        let call = with_instr_id(
            InstrBlockPy::Call(Call::new(local("coro", 0), Vec::new(), Vec::new())),
            9,
        );
        let mut caller = function_with_name_gen(
            module_name_gen.next_function_name_gen(),
            "caller",
            "caller",
            vec![Block::new(
                label(0),
                vec![InstrBlockPy::Store(Store::new(
                    local_name("result", 1),
                    call,
                ))],
                BlockTerm::Return(local("result", 1)),
                Vec::<BlockParam>::new(),
                None,
            )],
        );
        set_stack_slots(&mut caller, &["coro", "result"]);
        let mut coroutine = simple_arg_return_callee(&[("_dp_self", false)]);
        coroutine.names = FunctionName::new("coro", "coro", "coro", "coro");
        coroutine.kind = FunctionKind::Coroutine;
        let caller_id = caller.function_id;
        let coroutine_id = coroutine.function_id;
        let module = BlockPyModule {
            strict_source: None,
            module_name_gen,
            global_names: Vec::new(),
            callable_defs: vec![caller.clone(), coroutine],
            module_constants: Vec::new(),
            counter_defs: Vec::new(),
        };
        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows: vec![row(
                "call_hot_targets",
                caller_id,
                source,
                1,
                Some(coroutine_id.to_packed_runtime_u64()),
            )],
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);
        let module_identity = module_identity();
        let target_index = DirectCallTargetIndex::from_current_module(&module_identity, &module);
        let mut identity_builder = OptimizationPlanV3IdentityBuilder::new(&module_identity);

        let (requests, diagnostics) = direct_call_requests_from_evidence_v3(
            &module,
            &module_identity,
            &caller,
            &evidence_store,
            &target_index,
            &mut identity_builder,
        );

        assert!(requests.is_empty(), "{requests:?}");
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0]
                .message
                .contains("generator-like targets require public factory semantics"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn direct_call_requests_include_method_targets_with_implicit_receiver() {
        let module_name_gen = ModuleNameGen::new(0);
        let source = instr_id(9);
        let call = with_instr_id(
            InstrBlockPy::Call(Call::new(
                InstrBlockPy::GetAttr(GetAttr::new(local("record", 0), constant_name(0))),
                Vec::new(),
                Vec::new(),
            )),
            9,
        );
        let mut caller = function_with_name_gen(
            module_name_gen.next_function_name_gen(),
            "caller",
            "caller",
            vec![Block::new(
                label(0),
                vec![InstrBlockPy::Store(Store::new(
                    local_name("result", 1),
                    call,
                ))],
                BlockTerm::Return(local("result", 1)),
                Vec::<BlockParam>::new(),
                None,
            )],
        );
        set_stack_slots(&mut caller, &["record", "result"]);
        let mut method = function_with_name_gen(
            module_name_gen.next_function_name_gen(),
            "copy",
            "Record.copy",
            vec![Block::new(
                label(0),
                Vec::new(),
                BlockTerm::Return(local("self", 0)),
                Vec::<BlockParam>::new(),
                None,
            )],
        );
        method.params.params = vec![any_param("self", false)];
        set_stack_slots(&mut method, &["self"]);
        let caller_id = caller.function_id;
        let method_id = method.function_id;
        let module = BlockPyModule {
            strict_source: None,
            module_name_gen,
            global_names: Vec::new(),
            callable_defs: vec![caller.clone(), method],
            module_constants: vec![ConstantExpr::Literal(LiteralValue::new(StringLiteral {
                value: "copy".to_string(),
            }))],
            counter_defs: Vec::new(),
        };
        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows: vec![row(
                "call_hot_targets",
                caller_id,
                source,
                1,
                Some(method_id.to_packed_runtime_u64()),
            )],
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);
        let module_identity = module_identity();
        let target_index = DirectCallTargetIndex::from_current_module(&module_identity, &module);
        let mut identity_builder = OptimizationPlanV3IdentityBuilder::new(&module_identity);

        let (requests, diagnostics) = direct_call_requests_from_evidence_v3(
            &module,
            &module_identity,
            &caller,
            &evidence_store,
            &target_index,
            &mut identity_builder,
        );

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, source);
        assert_eq!(
            requests[0].callee,
            DirectCallCallee::Method {
                method_name: "copy".to_string()
            }
        );
        assert_eq!(
            requests[0].arg_plan.sources,
            vec![DirectCallArgSource::Provided(0)]
        );
        assert!(
            requests[0]
                .body
                .alternatives
                .iter()
                .any(|alternative| alternative.kind == CallBodyKind::Inline)
        );
        assert!(requests[0].reason.contains("method"));
    }

    #[test]
    fn direct_call_requests_include_runtime_next_protocol_targets() {
        let module_name_gen = ModuleNameGen::new(0);
        let source = instr_id(10);
        let call = with_instr_id(
            InstrBlockPy::Call(Call::new(
                runtime_name(RuntimeName::Next),
                vec![CallArgPositional::Positional(local("it", 0))],
                Vec::new(),
            )),
            10,
        );
        let mut caller = function_with_name_gen(
            module_name_gen.next_function_name_gen(),
            "caller",
            "caller",
            vec![Block::new(
                label(0),
                vec![InstrBlockPy::Store(Store::new(
                    local_name("result", 1),
                    call,
                ))],
                BlockTerm::Return(local("result", 1)),
                Vec::<BlockParam>::new(),
                None,
            )],
        );
        set_stack_slots(&mut caller, &["it", "result"]);
        let mut next_method = function_with_name_gen(
            module_name_gen.next_function_name_gen(),
            "__next__",
            "IterRange.__next__",
            vec![Block::new(
                label(0),
                Vec::new(),
                BlockTerm::Return(local("self", 0)),
                Vec::<BlockParam>::new(),
                None,
            )],
        );
        next_method.params.params = vec![any_param("self", false)];
        set_stack_slots(&mut next_method, &["self"]);
        let caller_id = caller.function_id;
        let next_id = next_method.function_id;
        let module = BlockPyModule {
            strict_source: None,
            module_name_gen,
            global_names: Vec::new(),
            callable_defs: vec![caller.clone(), next_method],
            module_constants: Vec::new(),
            counter_defs: Vec::new(),
        };
        let record = CounterDumpRecord {
            source_hash: 0x99,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows: vec![row(
                "call_hot_targets",
                caller_id,
                source,
                1,
                Some(next_id.to_packed_runtime_u64()),
            )],
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);
        let module_identity = module_identity();
        let target_index = DirectCallTargetIndex::from_current_module(&module_identity, &module);
        let mut identity_builder = OptimizationPlanV3IdentityBuilder::new(&module_identity);

        let (requests, diagnostics) = direct_call_requests_from_evidence_v3(
            &module,
            &module_identity,
            &caller,
            &evidence_store,
            &target_index,
            &mut identity_builder,
        );

        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].callee,
            DirectCallCallee::RuntimeProtocolMethod {
                runtime_name: RuntimeName::Next,
                method_name: "__next__".to_string()
            }
        );
        assert_eq!(
            requests[0].arg_plan.sources,
            vec![DirectCallArgSource::Provided(0)]
        );
        assert!(
            requests[0]
                .body
                .alternatives
                .iter()
                .any(|alternative| alternative.kind == CallBodyKind::Inline)
        );
        assert!(requests[0].reason.contains("runtime protocol method"));
    }

    #[test]
    fn exact_list_item_requests_are_derived_from_raw_shape_evidence() {
        let get_source = InstrId::new(5);
        let set_source = InstrId::new(8);
        let block = Block::new(
            label(0),
            vec![
                InstrBlockPy::GetItem(GetItem::new(local("items", 0), local("index", 1)))
                    .with_meta(Meta {
                        instr_id: Some(get_source),
                        ..Meta::synthetic()
                    }),
                InstrBlockPy::SetItem(SetItem::new(
                    local("items", 0),
                    local("index", 1),
                    local("value", 2),
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
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .getitem_specializations
            .insert(get_source, vec![EXACT_LIST_EXACT_INT_ITEM_SHAPE_TAG]);
        evidence
            .setitem_specializations
            .insert(set_source, vec![EXACT_LIST_EXACT_INT_ITEM_SHAPE_TAG]);

        let (requests, diagnostics) =
            exact_list_item_requests_from_profile_evidence_v3(&function, &evidence);

        assert!(diagnostics.is_empty());
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].source, get_source);
        assert_eq!(requests[0].access, ExactListItemAccessKind::Get);
        assert_eq!(requests[0].shape, ExactListItemShape::ExactListExactInt);
        assert_eq!(requests[1].source, set_source);
        assert_eq!(requests[1].access, ExactListItemAccessKind::Set);
        assert_eq!(requests[1].shape, ExactListItemShape::ExactListExactInt);
    }

    fn exact_float_sum_tree_for_test() -> InstrBlockPy {
        let first = binary(
            BinOpKind::Mul,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("a", 0), 1),
            2,
        );
        let second = binary(
            BinOpKind::Mul,
            with_instr_id(local("b", 1), 3),
            with_instr_id(local("b", 1), 4),
            5,
        );
        let prefix = binary(BinOpKind::Add, first, second, 6);
        let third = binary(
            BinOpKind::Mul,
            with_instr_id(local("c", 2), 7),
            with_instr_id(local("c", 2), 8),
            9,
        );
        binary(BinOpKind::Add, prefix, third, 10)
    }

    fn exact_float_sum_evidence_for_test() -> FunctionProfileEvidence {
        let mut evidence = FunctionProfileEvidence::default();
        let shape = pack_binary_shape(ExactTypeTag::Float, ExactTypeTag::Float);
        for source in [2, 5, 6, 9, 10] {
            evidence
                .operator_specializations
                .insert(instr_id(source), vec![shape]);
        }
        evidence
    }

    #[test]
    fn exact_float_expression_plans_select_maximal_trees_under_calls_and_powers() {
        for wrap_in_call in [true, false] {
            let tree = exact_float_sum_tree_for_test();
            let wrapped = if wrap_in_call {
                with_instr_id(
                    InstrBlockPy::Call(Call::new(
                        local("sink", 3),
                        vec![CallArgPositional::Positional(tree)],
                        Vec::new(),
                    )),
                    12,
                )
            } else {
                binary(
                    BinOpKind::Pow,
                    tree,
                    with_instr_id(local("power", 4), 11),
                    12,
                )
            };
            let mut function = function_with_blocks(vec![Block::new(
                label(0),
                Vec::new(),
                BlockTerm::Return(wrapped),
                Vec::<BlockParam>::new(),
                None,
            )]);
            set_stack_slots(&mut function, &["a", "b", "c", "sink", "power"]);
            let evidence = exact_float_sum_evidence_for_test();
            let artifacts = plan_and_emit_function_exact_int_branches_v3_with_module_constants(
                &AlternativeCatalog::default_v3(),
                module_identity(),
                function_identity(),
                &function,
                &evidence,
                &[],
            )
            .expect("nested exact-float trees should produce a valid v3 plan");

            let selected = &artifacts.plan.functions[0].exact_float_expressions;
            assert_eq!(
                selected.len(),
                1,
                "only the maximal tree should be selected"
            );
            assert_eq!(selected[0].source, instr_id(10));
            assert_eq!(
                selected[0]
                    .operations
                    .iter()
                    .map(|operation| (operation.source, operation.kind))
                    .collect::<Vec<_>>(),
                vec![
                    (instr_id(2), BinOpKind::Mul),
                    (instr_id(5), BinOpKind::Mul),
                    (instr_id(6), BinOpKind::Add),
                    (instr_id(9), BinOpKind::Mul),
                    (instr_id(10), BinOpKind::Add),
                ]
            );
            assert_eq!(
                artifacts.emission.functions[0].exact_float_expressions, *selected,
                "mechanical emission must preserve the complete source-keyed decision"
            );
        }
    }

    #[test]
    fn exact_float_expression_plans_reject_single_mixed_and_side_effectful_trees() {
        let shape = pack_binary_shape(ExactTypeTag::Float, ExactTypeTag::Float);
        let single = binary(
            BinOpKind::Mul,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .operator_specializations
            .insert(instr_id(2), vec![shape]);
        assert!(exact_float_expression_plan_for_instr_v3(&single, Some(&evidence)).is_none());

        let tree = exact_float_sum_tree_for_test();
        let mut mixed_evidence = exact_float_sum_evidence_for_test();
        mixed_evidence.operator_specializations.insert(
            instr_id(5),
            vec![pack_binary_shape(ExactTypeTag::Int, ExactTypeTag::Float)],
        );
        assert!(
            exact_float_expression_plan_for_instr_v3(&tree, Some(&mixed_evidence)).is_none(),
            "every arithmetic node must have exact-float profile evidence"
        );

        let effectful = binary(
            BinOpKind::Add,
            single,
            with_instr_id(
                InstrBlockPy::Call(Call::new(local("effect", 2), Vec::new(), Vec::new())),
                3,
            ),
            4,
        );
        evidence
            .operator_specializations
            .insert(instr_id(4), vec![shape]);
        assert!(
            exact_float_expression_plan_for_instr_v3(&effectful, Some(&evidence)).is_none(),
            "calls and other observable operand evaluation must retain generic evaluation"
        );
    }

    #[test]
    fn exact_float_expression_linearization_preserves_only_selected_atomic_trees() {
        let root = exact_float_sum_tree_for_test();
        let wrapped = with_instr_id(
            InstrBlockPy::Call(Call::new(
                local("sink", 3),
                vec![CallArgPositional::Positional(root.clone())],
                Vec::new(),
            )),
            12,
        );
        let mut function = function_with_blocks(vec![Block::new(
            label(0),
            Vec::new(),
            BlockTerm::Return(wrapped),
            Vec::<BlockParam>::new(),
            None,
        )]);
        set_stack_slots(&mut function, &["a", "b", "c", "sink"]);
        let evidence = exact_float_sum_evidence_for_test();
        let plan = exact_float_expression_plan_for_instr_v3(&root, Some(&evidence))
            .expect("profile should select the five-operation expression");
        let mut selected = lower_blockpy_function_to_typed(function.clone());
        let mut ordinary = lower_blockpy_function_to_typed(function);

        struct Annotator {
            plan: ExactFloatExpressionSpecializationPlan,
        }

        impl VisitMut<InstrTyped> for Annotator {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if let InstrTyped::BinOp(op) = expr
                    && op.try_semantic_instr_id() == Some(self.plan.source)
                {
                    op.extra_mut()
                        .set_exact_float_expression_plan(TypedExactFloatExpressionPlan {
                            source: self.plan.source,
                            operations: self.plan.operations.clone(),
                            leaf_sources: self.plan.leaf_sources.clone(),
                        });
                    return;
                }
                expr.visit_children_mut(self);
            }
        }

        let expected_operations = plan.operations.clone();
        Annotator { plan }.visit_fn_mut(&mut selected);
        crate::passes::linearize_typed_function_expressions(&mut selected)
            .expect("selected arithmetic should linearize as one opaque expression");
        crate::passes::linearize_typed_function_expressions(&mut ordinary)
            .expect("unselected arithmetic should retain ordinary linearization");

        // The callable must be resolved before arithmetic, including the
        // selected tree's generic fallback. An unbound sink must fail before
        // an operand overload can run; rebinding sink in that overload must
        // not replace the callable already captured by this call.
        for function in [&selected, &ordinary] {
            let block = &function.blocks[0];
            let Some(InstrTyped::Store(callable)) = block.body.first() else {
                panic!("the callable should be captured before arithmetic");
            };
            assert!(matches!(
                callable.value.as_ref(),
                InstrTyped::Load(load) if load.name == local_name("sink", 3)
            ));
            let (call_result, call) = block
                .body
                .iter()
                .find_map(|instr| {
                    let InstrTyped::Store(store) = instr else {
                        return None;
                    };
                    let InstrTyped::CallTyped(call) = store.value.as_ref() else {
                        return None;
                    };
                    Some((store, call))
                })
                .expect("the captured sink call should produce the return value");
            assert!(matches!(
                call.func.as_ref(),
                InstrTyped::Load(load) if load.name == callable.name
            ));
            let [CallArgPositional::Positional(InstrTyped::Load(argument))] = call.args.as_slice()
            else {
                panic!("the argument should be a captured arithmetic result")
            };
            assert!(block.body.iter().any(|instr| matches!(
                instr, InstrTyped::Store(store)
                    if store.name == argument.name && matches!(store.value.as_ref(), InstrTyped::BinOp(_))
            )));
            assert!(
                matches!(&block.term, BlockTerm::Return(InstrTyped::Load(load))
                if load.name == call_result.name)
            );
        }

        let selected_arithmetic = selected.blocks[0]
            .body
            .iter()
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                matches!(store.value.as_ref(), InstrTyped::BinOp(_)).then_some(store)
            })
            .collect::<Vec<_>>();
        let [expression] = selected_arithmetic.as_slice() else {
            panic!("the selected tree should remain one arithmetic expression");
        };
        let selected_plan = expression
            .value
            .exact_float_expression_plan()
            .expect("the complete arithmetic tree should retain its selected plan");
        assert_eq!(selected_plan.operations, expected_operations);

        struct ArithmeticCollector {
            operations: Vec<ExactFloatExpressionOperationPlan>,
        }

        impl Visit<InstrTyped> for ArithmeticCollector {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                expr.visit_children(self);
                if let InstrTyped::BinOp(op) = expr {
                    self.operations.push(ExactFloatExpressionOperationPlan {
                        source: op.semantic_instr_id(),
                        kind: op.kind,
                    });
                }
            }
        }

        let mut collector = ArithmeticCollector {
            operations: Vec::new(),
        };
        collector.visit_instr(&expression.value);
        assert_eq!(collector.operations, expected_operations);

        let ordinary_operations = ordinary.blocks[0]
            .body
            .iter()
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::BinOp(op) = store.value.as_ref() else {
                    return None;
                };
                assert!(op.extra().exact_float_expression_plan().is_none());
                assert!(matches!(op.left.as_ref(), InstrTyped::Load(_)));
                assert!(matches!(op.right.as_ref(), InstrTyped::Load(_)));
                Some(ExactFloatExpressionOperationPlan {
                    source: op.semantic_instr_id(),
                    kind: op.kind,
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(ordinary_operations, expected_operations);
    }

    #[test]
    fn exact_tuple_get_requests_are_derived_from_raw_shape_evidence() {
        let get_source = InstrId::new(5);
        let block = Block::new(
            label(0),
            vec![
                InstrBlockPy::GetItem(GetItem::new(local("items", 0), local("index", 1)))
                    .with_meta(Meta {
                        instr_id: Some(get_source),
                        ..Meta::synthetic()
                    }),
            ],
            BlockTerm::jump_term(label(1)),
            Vec::<BlockParam>::new(),
            None,
        );
        let function = function_with_blocks(vec![block]);
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .getitem_specializations
            .insert(get_source, vec![EXACT_TUPLE_EXACT_INT_ITEM_SHAPE_TAG]);

        let (requests, diagnostics) =
            exact_list_item_requests_from_profile_evidence_v3(&function, &evidence);

        assert!(diagnostics.is_empty());
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].source, get_source);
        assert_eq!(requests[0].access, ExactListItemAccessKind::Get);
        assert_eq!(requests[0].shape, ExactListItemShape::ExactTupleExactInt);
    }

    #[test]
    fn exact_tuple_set_shape_is_not_selected_from_profile_evidence() {
        let set_source = InstrId::new(8);
        let block = Block::new(
            label(0),
            vec![
                InstrBlockPy::SetItem(SetItem::new(
                    local("items", 0),
                    local("index", 1),
                    local("value", 2),
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
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .setitem_specializations
            .insert(set_source, vec![EXACT_TUPLE_EXACT_INT_ITEM_SHAPE_TAG]);

        let (requests, diagnostics) =
            exact_list_item_requests_from_profile_evidence_v3(&function, &evidence);

        assert!(requests.is_empty());
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("setitem_hot_shapes"));
    }

    #[test]
    fn indexed_field_requests_are_derived_from_raw_type_key_evidence() {
        let attr_name = constant_name(0);
        let get_source = InstrId::new(5);
        let set_source = InstrId::new(8);
        let block = Block::new(
            label(0),
            vec![
                InstrBlockPy::GetAttr(GetAttr::new(local("record", 0), attr_name.clone()))
                    .with_meta(Meta {
                        instr_id: Some(get_source),
                        ..Meta::synthetic()
                    }),
                InstrBlockPy::SetAttr(SetAttr::new(
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
            strict_source: None,
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function],
            module_constants: vec![ConstantExpr::Literal(LiteralValue::new(StringLiteral {
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
    fn indexed_global_requests_are_derived_from_raw_module_key_evidence() {
        let load_source = InstrId::new(5);
        let store_source = InstrId::new(8);
        let block = Block::new(
            label(0),
            vec![
                InstrBlockPy::Load(Load::new(global_name("counter", 1))).with_meta(Meta {
                    instr_id: Some(load_source),
                    ..Meta::synthetic()
                }),
                InstrBlockPy::Store(Store::new(global_name("counter", 1), local("value", 0)))
                    .with_meta(Meta {
                        instr_id: Some(store_source),
                        ..Meta::synthetic()
                    }),
                InstrBlockPy::Load(Load::new(global_name("other", 2))).with_meta(Meta {
                    instr_id: Some(InstrId::new(11)),
                    ..Meta::synthetic()
                }),
            ],
            BlockTerm::jump_term(label(1)),
            Vec::<BlockParam>::new(),
            None,
        );
        let function = function_with_blocks(vec![block]);
        let record = CounterDumpRecord {
            source_hash: 0x1234,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows: Vec::new(),
            module_keys: vec![
                CounterDumpKeyLayout {
                    owner: "pkg.mod".to_string(),
                    key: "counter".to_string(),
                    index: 1,
                },
                CounterDumpKeyLayout {
                    owner: "pkg.mod".to_string(),
                    key: "other".to_string(),
                    index: 99,
                },
            ],
            type_keys: Vec::new(),
            type_table: Vec::new(),
        };
        let path = unique_counter_path_v3();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);

        let requests = indexed_global_requests_from_module_key_evidence_v3(
            "pkg.mod",
            &function,
            &evidence_store,
        );

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].source, load_source);
        assert_eq!(requests[0].access, IndexedGlobalAccessKind::Load);
        assert_eq!(requests[0].module_name, "pkg.mod");
        assert_eq!(requests[0].name, "counter");
        assert_eq!(requests[0].expected_index, 1);
        assert_eq!(requests[1].source, store_source);
        assert_eq!(requests[1].access, IndexedGlobalAccessKind::Store);
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
