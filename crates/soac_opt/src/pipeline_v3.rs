use crate::alternatives_v3::AlternativeCatalog;
use crate::artifacts_v3::ExactIntBranchV3Artifacts;
use crate::evidence_v3::{
    PlannerFactHints, planner_fact_hints_from_module_constants_v3,
    planner_facts_from_profile_evidence_v3,
};
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
    BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, Call, CallArgPositional, ChildVisitable,
    ConstantExpr, FunctionExecutionMode, FunctionKind, HasSemanticInstrId, InstrId,
    LocalFunctionId, LocalLocation, ModuleContentId, NameLike, NameLocation, ParamKind,
    PersistentFunctionId, ResolvedName, RuntimeName, SerializedFunctionDebugName,
    SerializedFunctionId, SerializedIdentityTables, SerializedModuleId, SerializedModuleIdentity,
    Tuple, Visit,
};
use soac_ir_blockpy::is_constructor_entry_function;
use soac_ir_typed::emit_v3::{MechanicalEmitError, emit_mechanical_plan_v3};
use soac_ir_typed::plan_v3::{
    DirectCallArgPlan, DirectCallArgSource, DirectCallCallee, DirectCallSpecializationPlan,
    EXACT_LIST_EXACT_INT_ITEM_SHAPE_TAG, ExactListItemAccessKind, ExactListItemShape,
    ExactListItemSpecializationPlan, FunctionOptimizationPlanV3, FunctionPlanIdentity,
    IndexedFieldAccessKind, IndexedFieldOwnerType, IndexedFieldSpecializationPlan,
    IndexedGlobalAccessKind, IndexedGlobalSpecializationPlan, ModuleOptimizationPlanV3,
    ModulePlanIdentity, PlanDiagnostic, RegionId,
};
use std::collections::HashMap;
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
        functions.push(FunctionPlanRequest {
            function: function_plan_identity_v3(function),
            regions: region_requests,
            direct_calls,
            exact_list_items,
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
    for (function, diagnostics) in plan.functions.iter_mut().zip(diagnostics_by_function) {
        function.diagnostics.extend(diagnostics);
    }
    validate_module_plan_v3_against_lowered_module(&plan, lowered_module)
        .map_err(ExactIntBranchV3Error::Emit)?;
    let emission = emit_mechanical_plan_v3(&plan).map_err(ExactIntBranchV3Error::Emit)?;
    Ok(ExactIntBranchV3Artifacts { plan, emission })
}

fn function_uses_generator_resume_state(function: &BlockPyFunction<BlockPyModuleShape>) -> bool {
    function.storage_layout.as_ref().is_some_and(|layout| {
        layout
            .freevars
            .iter()
            .any(|slot| slot.logical_name == "_dp_pc")
            || layout
                .cellvars
                .iter()
                .any(|slot| slot.logical_name == "_dp_pc")
            || layout
                .preserved_slots
                .iter()
                .any(|slot| slot.logical_name == "_dp_pc")
    })
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
    if plan.shape != ExactListItemShape::ExactListExactInt {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "function {} exact-list item shape {:?} for {}, but codegen only supports ExactListExactInt",
            planned_function.function.function, plan.shape, plan.source
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
    let Some((return_target, call)) =
        inline_call_and_return_target_for_instr_id_v3(function, source)
    else {
        return false;
    };
    let target_function = &target.function;
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
) -> Option<(InlineCallReturnTargetV3, &Call<InstrBlockPy>)> {
    for block in &function.blocks {
        for instr in &block.body {
            match instr {
                InstrBlockPy::Store(store) => {
                    let InstrBlockPy::Call(call) = store.value.as_ref() else {
                        continue;
                    };
                    if call.try_semantic_instr_id() == Some(source) {
                        return Some((InlineCallReturnTargetV3::StoreTo(store.name.clone()), call));
                    }
                }
                InstrBlockPy::Call(call) if call.try_semantic_instr_id() == Some(source) => {
                    return Some((InlineCallReturnTargetV3::Discard, call));
                }
                _ => {}
            }
        }
        if let BlockTerm::Return(InstrBlockPy::Call(call)) = &block.term
            && call.try_semantic_instr_id() == Some(source)
        {
            return Some((InlineCallReturnTargetV3::Discard, call));
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
    use crate::operator_specialization::{ExactTypeTag, pack_binary_shape};
    use crate::region_v3::{ExtractedValueId, extract_block_region_v3};
    use soac_core::block_py::literal::{LiteralValue, StringLiteral};
    use soac_core::block_py::{
        BinOp, BinOpKind, Block, BlockLabel, BlockParam, BlockPyName, BlockTerm, Call,
        CallArgPositional, FunctionName, GetAttr, GetItem, InstrId, Load, LocalFunctionId,
        LocalLocation, Meta, ModuleNameGen, NameLocation, Param, ParamSpec, ResolvedName,
        RuntimeFunctionId, SerializedFunctionId, SerializedModuleId, SetAttr, SetItem,
        StorageLayout, Store, TermIf, WithMeta,
    };
    use soac_core::profile::{
        CounterDumpKeyLayout, CounterDumpRecord, CounterDumpRow, CounterDumpTypeKey,
        CounterDumpTypeKeyLayout, CounterDumpTypeTableEntry,
    };
    use soac_ir_blockpy::{
        CONSTRUCTOR_ENTRY_FUNCTION_NAME, CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME, InstrBlockPy,
    };
    use soac_ir_typed::plan_v3::{CallBodyKind, RegionId, validate_module_plan_v3};
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
