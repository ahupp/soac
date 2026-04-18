use crate::counter_dump::{CounterDumpFile, collect_type_key_layouts, collect_type_table};
use anyhow::{Context, Result, bail};
use soac_blockpy::block_py::Literal;
use soac_blockpy::codegen_cache::{
    CachedCodegenModule, CachedCodegenModuleMetadata, PythonModuleCacheSource,
    load_codegen_module_cache, module_optimization_plan_path,
};
use soac_blockpy::passes::{CodegenModuleShape, InstrCodegen, InstrResolved};
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, ChildVisitable, HasSemanticInstrId, InstrId, LocalFunctionId,
    ModuleContentId, NameLocation, PersistentFunctionId, RuntimeFunctionId, RuntimeModuleId,
    SerializedFunctionDebugName, SerializedFunctionId, SerializedIdentityTables,
    SerializedModuleId, SerializedModuleIdentity, Visit,
};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionProfileEvidence {
    pub call_target_specializations: HashMap<InstrId, Vec<RuntimeFunctionId>>,
    pub operator_specializations: HashMap<InstrId, Vec<u64>>,
    pub getitem_specializations: HashMap<InstrId, Vec<u64>>,
    pub setitem_specializations: HashMap<InstrId, Vec<u64>>,
    pub field_index_specializations: HashMap<InstrId, Vec<PlannedIndexedFieldSpecialization>>,
    pub branch_prefer_true: HashMap<InstrId, bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PersistentFunctionProfileEvidence {
    call_target_specializations: HashMap<InstrId, Vec<PersistentFunctionId>>,
    operator_specializations: HashMap<InstrId, Vec<u64>>,
    getitem_specializations: HashMap<InstrId, Vec<u64>>,
    setitem_specializations: HashMap<InstrId, Vec<u64>>,
    field_index_specializations: HashMap<InstrId, Vec<PlannedIndexedFieldSpecialization>>,
    branch_prefer_true: HashMap<InstrId, bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ProfileEvidenceStore {
    functions: HashMap<PersistentFunctionId, PersistentFunctionProfileEvidence>,
    module_source_hashes: HashMap<String, u64>,
    function_targets: HashMap<RuntimeFunctionId, PersistentFunctionId>,
    module_targets_by_runtime_id: HashMap<RuntimeModuleId, PlannedModuleTarget>,
    ambiguous_module_runtime_ids: HashSet<RuntimeModuleId>,
    field_index_specializations_by_attr: HashMap<String, Vec<PlannedIndexedFieldSpecialization>>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct OptimizationPlan {
    pub source: PythonModuleCacheSource,
    pub module_name: String,
    pub source_hash: u64,
    pub cache_identity: String,
    pub identity_tables: SerializedIdentityTables,
    pub functions: Vec<FunctionOptimizationPlan>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FunctionOptimizationPlan {
    pub function: SerializedFunctionId,
    pub decisions: Vec<OptimizationDecision>,
}

impl FunctionOptimizationPlan {
    pub const fn local_function_id(&self) -> LocalFunctionId {
        self.function.local_function_id()
    }

    pub const fn runtime_function_id(&self, module_id: RuntimeModuleId) -> RuntimeFunctionId {
        RuntimeFunctionId::new(module_id, self.local_function_id())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct OptimizationDecision {
    pub instr_id: InstrId,
    pub replacement: PlannedReplacement,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PlannedReplacement {
    Guarded {
        alternatives: Vec<PlannedAlternative>,
        fallback: PlannedFallback,
    },
    BranchPreference {
        prefer_true: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PlannedAlternative {
    pub guards: Vec<PlannedGuard>,
    pub action: PlannedAction,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PlannedGuard {
    FunctionTarget {
        target: PlannedFunctionTarget,
    },
    ObservedShape {
        family: ShapeFamily,
        shape: u64,
    },
    IndexedField {
        specialization: PlannedIndexedFieldSpecialization,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PlannedAction {
    DirectCall {
        target: PlannedFunctionTarget,
    },
    SpecializedShape {
        family: ShapeFamily,
        shape: u64,
    },
    IndexedField {
        specialization: PlannedIndexedFieldSpecialization,
    },
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct PlannedFunctionTarget {
    pub function: SerializedFunctionId,
}

impl PlannedFunctionTarget {
    pub const fn new(function: SerializedFunctionId) -> Self {
        Self { function }
    }

    pub const fn local_function_id(&self) -> LocalFunctionId {
        self.function.local_function_id()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlannedModuleTarget {
    module_name: String,
    source_hash: u64,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct PlannedIndexedFieldSpecialization {
    pub owner_type: PlannedTypeKey,
    pub attr_name: String,
    pub expected_index: u32,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct PlannedTypeKey {
    pub module_name: String,
    pub qualname: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PlannedFallback {
    OriginalInstruction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ShapeFamily {
    Operator,
    GetItem,
    SetItem,
}

impl ProfileEvidenceStore {
    pub fn from_counter_dump(path: &Path) -> Result<Self> {
        let dump = CounterDumpFile::open(path)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("open counter dump {}", path.display()))?;
        let records = dump
            .records()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("read counter dump records from {}", path.display()))?;
        let mut store = Self::default();
        let mut branch_counts = HashMap::<(PersistentFunctionId, InstrId), [u64; 2]>::new();

        for record in &records {
            let module_name = record
                .module_name()
                .map_err(anyhow::Error::msg)?
                .to_string();
            if let Some(previous_source_hash) = store
                .module_source_hashes
                .insert(module_name.clone(), record.source_hash())
                .filter(|previous_source_hash| *previous_source_hash != record.source_hash())
            {
                bail!(
                    "counter dump contains module {module_name} with multiple source hashes: 0x{previous_source_hash:016x} and 0x{:016x}",
                    record.source_hash()
                );
            }
            for row_index in 0..record.row_count() {
                let row = record.row(row_index).map_err(anyhow::Error::msg)?;
                let Some(function_id) = row.function_id else {
                    continue;
                };
                store.record_function_target(
                    function_id,
                    module_name.as_str(),
                    record.source_hash(),
                );
                if let Some(current_function_id) = row.current_function_id {
                    store.record_function_target(
                        current_function_id,
                        module_name.as_str(),
                        record.source_hash(),
                    );
                }
            }
        }

        for record in &records {
            let module_name = record
                .module_name()
                .map_err(anyhow::Error::msg)?
                .to_string();
            for row_index in 0..record.row_count() {
                let row = record.row(row_index).map_err(anyhow::Error::msg)?;
                let Some(function_id) = row.function_id else {
                    continue;
                };
                let function_id = persistent_function_id_for_counter_row(
                    module_name.as_str(),
                    record.source_hash(),
                    function_id,
                );
                let Some(instr_id) = row.instr_id else {
                    continue;
                };
                match row.kind {
                    "call_hot_targets" => {
                        let Some(observed_value) = row.observed_value else {
                            continue;
                        };
                        if observed_value == 0 {
                            continue;
                        }
                        let observed = RuntimeFunctionId::from_packed_runtime_u64(observed_value);
                        if observed == RuntimeFunctionId::global() {
                            continue;
                        }
                        let Some(observed) = store.function_target(observed) else {
                            continue;
                        };
                        let function = store.functions.entry(function_id.clone()).or_default();
                        push_unique(
                            function
                                .call_target_specializations
                                .entry(instr_id)
                                .or_default(),
                            observed,
                        );
                    }
                    "operator_hot_shapes" => {
                        let function = store.functions.entry(function_id.clone()).or_default();
                        push_observed_shape(
                            &mut function.operator_specializations,
                            instr_id,
                            row.observed_value,
                        );
                    }
                    "getitem_hot_shapes" => {
                        let function = store.functions.entry(function_id.clone()).or_default();
                        push_observed_shape(
                            &mut function.getitem_specializations,
                            instr_id,
                            row.observed_value,
                        );
                    }
                    "setitem_hot_shapes" => {
                        let function = store.functions.entry(function_id.clone()).or_default();
                        push_observed_shape(
                            &mut function.setitem_specializations,
                            instr_id,
                            row.observed_value,
                        );
                    }
                    "branch_outcomes" => {
                        let Some(slot) = row
                            .observed_value
                            .and_then(|value| usize::try_from(value).ok())
                        else {
                            continue;
                        };
                        if slot < 2 {
                            let counts = branch_counts
                                .entry((function_id.clone(), instr_id))
                                .or_default();
                            counts[slot] = counts[slot].saturating_add(row.value);
                        }
                    }
                    _ => {}
                }
            }
        }

        let type_table = collect_type_table(records.as_slice()).map_err(anyhow::Error::msg)?;
        let type_key_layouts =
            collect_type_key_layouts(records.as_slice()).map_err(anyhow::Error::msg)?;
        for (type_id, layouts) in type_key_layouts {
            let Some(type_key) = type_table.get(&type_id) else {
                continue;
            };
            for layout in layouts {
                let specialization = PlannedIndexedFieldSpecialization {
                    owner_type: PlannedTypeKey {
                        module_name: type_key.module_name.clone(),
                        qualname: type_key.qualname.clone(),
                    },
                    attr_name: layout.key,
                    expected_index: layout.index,
                };
                push_unique(
                    store
                        .field_index_specializations_by_attr
                        .entry(specialization.attr_name.clone())
                        .or_default(),
                    specialization,
                );
            }
        }

        for ((function_id, instr_id), [false_count, true_count]) in branch_counts {
            if false_count == 0 && true_count == 0 {
                continue;
            }
            store
                .functions
                .entry(function_id)
                .or_default()
                .branch_prefer_true
                .insert(instr_id, true_count >= false_count);
        }

        Ok(store)
    }

    fn for_function(
        &self,
        module_name: &str,
        source_hash: u64,
        function_id: RuntimeFunctionId,
    ) -> PersistentFunctionProfileEvidence {
        let function_id =
            persistent_function_id_for_counter_row(module_name, source_hash, function_id);
        self.functions
            .get(&function_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn module_source_hash(&self, module_name: &str) -> Option<u64> {
        self.module_source_hashes.get(module_name).copied()
    }

    pub fn function_target(&self, function_id: RuntimeFunctionId) -> Option<PersistentFunctionId> {
        if function_id == RuntimeFunctionId::global()
            || self
                .ambiguous_module_runtime_ids
                .contains(&function_id.runtime_module_id())
        {
            return None;
        }
        self.function_targets
            .get(&function_id)
            .cloned()
            .or_else(|| {
                self.module_targets_by_runtime_id
                    .get(&function_id.runtime_module_id())
                    .map(|module_target| {
                        PersistentFunctionId::new(
                            ModuleContentId::new(
                                module_target.module_name.clone(),
                                module_target.source_hash,
                            ),
                            function_id.local_function_id(),
                        )
                    })
            })
    }

    pub fn field_index_specializations_for_attr(
        &self,
        attr_name: &str,
    ) -> Option<&[PlannedIndexedFieldSpecialization]> {
        self.field_index_specializations_by_attr
            .get(attr_name)
            .map(Vec::as_slice)
    }
}

fn persistent_function_id_for_counter_row(
    module_name: &str,
    source_hash: u64,
    function_id: RuntimeFunctionId,
) -> PersistentFunctionId {
    PersistentFunctionId::new(
        ModuleContentId::new(module_name, source_hash),
        function_id.local_function_id(),
    )
}

impl ProfileEvidenceStore {
    fn record_function_target(
        &mut self,
        function_id: RuntimeFunctionId,
        module_name: &str,
        source_hash: u64,
    ) {
        if function_id == RuntimeFunctionId::global() {
            return;
        }
        self.record_module_target(function_id.runtime_module_id(), module_name, source_hash);
        self.function_targets.entry(function_id).or_insert_with(|| {
            PersistentFunctionId::new(
                ModuleContentId::new(module_name, source_hash),
                function_id.local_function_id(),
            )
        });
    }

    fn record_module_target(
        &mut self,
        module_id: RuntimeModuleId,
        module_name: &str,
        source_hash: u64,
    ) {
        if self.ambiguous_module_runtime_ids.contains(&module_id) {
            return;
        }
        let target = PlannedModuleTarget {
            module_name: module_name.to_string(),
            source_hash,
        };
        match self.module_targets_by_runtime_id.get(&module_id) {
            Some(existing) if existing != &target => {
                self.module_targets_by_runtime_id.remove(&module_id);
                self.ambiguous_module_runtime_ids.insert(module_id);
            }
            Some(_) => {}
            None => {
                self.module_targets_by_runtime_id.insert(module_id, target);
            }
        }
    }
}

struct OptimizationPlanIdentityBuilder {
    tables: SerializedIdentityTables,
    modules_by_content: HashMap<ModuleContentId, SerializedModuleId>,
}

impl OptimizationPlanIdentityBuilder {
    fn new(metadata: &CachedCodegenModuleMetadata) -> Self {
        let mut builder = Self {
            tables: SerializedIdentityTables::default(),
            modules_by_content: HashMap::new(),
        };
        builder.intern_module(
            ModuleContentId::new(metadata.module_name.clone(), metadata.source_hash),
            Some(metadata.cache_identity.clone()),
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

impl OptimizationPlan {
    pub fn module_content_id(&self) -> ModuleContentId {
        ModuleContentId::new(self.module_name.clone(), self.source_hash)
    }

    pub fn persistent_function_id(
        &self,
        function: SerializedFunctionId,
    ) -> Result<PersistentFunctionId> {
        self.identity_tables
            .persistent_function_id(function)
            .with_context(|| format!("resolve optimization-plan function id {function}"))
    }

    pub fn debug_name_for_function(&self, function: SerializedFunctionId) -> Option<&str> {
        self.identity_tables.debug_name_for_function(function)
    }

    pub fn from_evidence(
        metadata: &CachedCodegenModuleMetadata,
        module: &BlockPyModule<CodegenModuleShape>,
        evidence_store: &ProfileEvidenceStore,
    ) -> Self {
        let mut identity_builder = OptimizationPlanIdentityBuilder::new(metadata);
        let current_module =
            ModuleContentId::new(metadata.module_name.clone(), metadata.source_hash);
        let mut functions = module
            .callable_defs
            .iter()
            .filter_map(|function| {
                let evidence = evidence_store.for_function(
                    metadata.module_name.as_str(),
                    metadata.source_hash,
                    function.function_id,
                );
                let serialized_function =
                    identity_builder.function_id_for_persistent(PersistentFunctionId::new(
                        current_module.clone(),
                        function.function_id.local_function_id(),
                    ));
                let decisions = decisions_for_function(
                    module,
                    function,
                    evidence_store,
                    &evidence,
                    |function_id| {
                        PlannedFunctionTarget::new(
                            identity_builder.function_id_for_persistent(function_id),
                        )
                    },
                );
                (!decisions.is_empty()).then(|| FunctionOptimizationPlan {
                    function: serialized_function,
                    decisions,
                })
            })
            .collect::<Vec<_>>();
        for function in &functions {
            identity_builder.add_debug_name(
                function.function,
                module
                    .callable_defs
                    .iter()
                    .find(|callable| {
                        callable.function_id.local_function_id() == function.local_function_id()
                    })
                    .map(|callable| callable.names.qualname.clone())
                    .unwrap_or_else(|| "<unknown>".to_string()),
            );
        }
        functions.sort_by_key(|function| function.local_function_id());
        Self {
            source: metadata.source,
            module_name: metadata.module_name.clone(),
            source_hash: metadata.source_hash,
            cache_identity: metadata.cache_identity.clone(),
            identity_tables: identity_builder.finish(),
            functions,
        }
    }

    pub fn validate_for_module(
        &self,
        source: Option<PythonModuleCacheSource>,
        module_name: &str,
        source_hash: u64,
        cache_identity: &str,
    ) -> Result<()> {
        if let Some(source) = source
            && self.source != source
        {
            bail!(
                "optimization plan source for module {module_name} is {:?}, expected {:?}",
                self.source,
                source
            );
        }
        if self.module_name != module_name {
            bail!(
                "optimization plan module name is {}, expected {module_name}",
                self.module_name
            );
        }
        if self.source_hash != source_hash {
            bail!(
                "optimization plan source hash for module {module_name} is 0x{:016x}, expected 0x{source_hash:016x}",
                self.source_hash
            );
        }
        if self.cache_identity != cache_identity {
            bail!(
                "optimization plan cache identity for module {module_name} is {}, expected {cache_identity}",
                self.cache_identity
            );
        }
        let current_module =
            self.identity_tables.modules.first().ok_or_else(|| {
                anyhow::anyhow!("optimization plan has no serialized module table")
            })?;
        if current_module.module_name != module_name || current_module.source_hash != source_hash {
            bail!(
                "optimization plan primary module is {} source_hash=0x{:016x}, expected {module_name} source_hash=0x{source_hash:016x}",
                current_module.module_name,
                current_module.source_hash
            );
        }
        Ok(())
    }

    pub fn evidence_for_local_function(
        &self,
        local_function_id: LocalFunctionId,
        call_target_resolver: impl Fn(&PersistentFunctionId) -> Result<Option<RuntimeFunctionId>>,
    ) -> Result<FunctionProfileEvidence> {
        let Some(function) = self
            .functions
            .iter()
            .find(|function| function.local_function_id() == local_function_id)
        else {
            return Ok(FunctionProfileEvidence::default());
        };
        let mut evidence = FunctionProfileEvidence::default();
        for decision in &function.decisions {
            self.apply_decision_to_evidence(decision, &mut evidence, &call_target_resolver)?;
        }
        Ok(evidence)
    }

    pub fn evidence_by_local_function(
        &self,
        call_target_resolver: impl Fn(&PersistentFunctionId) -> Result<Option<RuntimeFunctionId>>,
    ) -> Result<HashMap<LocalFunctionId, FunctionProfileEvidence>> {
        let mut out = HashMap::new();
        for function in &self.functions {
            let mut evidence = FunctionProfileEvidence::default();
            for decision in &function.decisions {
                self.apply_decision_to_evidence(decision, &mut evidence, &call_target_resolver)?;
            }
            out.insert(function.local_function_id(), evidence);
        }
        Ok(out)
    }

    fn apply_decision_to_evidence(
        &self,
        decision: &OptimizationDecision,
        evidence: &mut FunctionProfileEvidence,
        call_target_resolver: &impl Fn(&PersistentFunctionId) -> Result<Option<RuntimeFunctionId>>,
    ) -> Result<()> {
        match &decision.replacement {
            PlannedReplacement::Guarded {
                alternatives,
                fallback: PlannedFallback::OriginalInstruction,
            } => {
                for alternative in alternatives {
                    self.apply_alternative_to_evidence(
                        decision.instr_id,
                        alternative,
                        evidence,
                        call_target_resolver,
                    )?;
                }
            }
            PlannedReplacement::BranchPreference { prefer_true } => {
                evidence
                    .branch_prefer_true
                    .insert(decision.instr_id, *prefer_true);
            }
        }
        Ok(())
    }

    fn apply_alternative_to_evidence(
        &self,
        instr_id: InstrId,
        alternative: &PlannedAlternative,
        evidence: &mut FunctionProfileEvidence,
        call_target_resolver: &impl Fn(&PersistentFunctionId) -> Result<Option<RuntimeFunctionId>>,
    ) -> Result<()> {
        match &alternative.action {
            PlannedAction::DirectCall { target } => {
                validate_alternative_guard(
                    alternative,
                    |guard| matches!(guard, PlannedGuard::FunctionTarget { target: guarded } if guarded == target),
                    "direct-call alternative",
                )?;
                let persistent_target = self.persistent_function_id(target.function)?;
                if let Some(function_id) = call_target_resolver(&persistent_target)? {
                    push_unique(
                        evidence
                            .call_target_specializations
                            .entry(instr_id)
                            .or_default(),
                        function_id,
                    );
                }
            }
            PlannedAction::SpecializedShape { family, shape } => {
                validate_alternative_guard(
                    alternative,
                    |guard| matches!(guard, PlannedGuard::ObservedShape { family: guarded_family, shape: guarded_shape } if guarded_family == family && guarded_shape == shape),
                    "shape-specialization alternative",
                )?;
                push_unique(
                    shape_map_for_family(evidence, *family)
                        .entry(instr_id)
                        .or_default(),
                    *shape,
                );
            }
            PlannedAction::IndexedField { specialization } => {
                validate_alternative_guard(
                    alternative,
                    |guard| matches!(guard, PlannedGuard::IndexedField { specialization: guarded } if guarded == specialization),
                    "field-index alternative",
                )?;
                push_unique(
                    evidence
                        .field_index_specializations
                        .entry(instr_id)
                        .or_default(),
                    specialization.clone(),
                );
            }
        }
        Ok(())
    }
}

pub fn load_optimization_plan(path: &Path) -> Result<OptimizationPlan> {
    let bytes =
        fs::read(path).with_context(|| format!("read optimization plan {}", path.display()))?;
    rkyv::from_bytes::<OptimizationPlan, rkyv::rancor::Error>(bytes.as_slice())
        .map_err(|err| anyhow::anyhow!("deserialize optimization plan {}: {err}", path.display()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedModuleOptimizationInput {
    pub module_path: PathBuf,
    pub strict: bool,
}

impl CachedModuleOptimizationInput {
    pub fn new(module_path: PathBuf, strict: bool) -> Self {
        Self {
            module_path,
            strict,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleOptimizationPlanReport {
    pub output_path: PathBuf,
    pub module_name: String,
    pub source_hash: u64,
    pub function_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OptimizationPlanGenerationSummary {
    pub reports: Vec<ModuleOptimizationPlanReport>,
    pub skipped: usize,
}

impl OptimizationPlanGenerationSummary {
    pub fn written(&self) -> usize {
        self.reports.len()
    }
}

pub fn generate_optimization_plans_for_counter_dump(
    counters_path: &Path,
    module_root: &Path,
    out_root: &Path,
) -> Result<OptimizationPlanGenerationSummary> {
    let evidence_store = ProfileEvidenceStore::from_counter_dump(counters_path)?;
    let module_inputs = cached_module_paths_under_root(module_root)?
        .into_iter()
        .map(|module_path| CachedModuleOptimizationInput::new(module_path, false));
    generate_optimization_plans_for_cached_modules(&evidence_store, module_inputs, out_root)
}

pub fn generate_optimization_plans_for_cached_modules(
    evidence_store: &ProfileEvidenceStore,
    module_inputs: impl IntoIterator<Item = CachedModuleOptimizationInput>,
    out_root: &Path,
) -> Result<OptimizationPlanGenerationSummary> {
    let mut summary = OptimizationPlanGenerationSummary::default();
    for module_input in module_inputs {
        match generate_module_optimization_plan(
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

pub fn generate_module_optimization_plan(
    evidence_store: &ProfileEvidenceStore,
    module_path: &Path,
    out_root: &Path,
    strict: bool,
) -> Result<Option<ModuleOptimizationPlanReport>> {
    let cache = load_codegen_module_cache(module_path)
        .with_context(|| format!("load BlockPy module cache {}", module_path.display()))?;
    if !counter_evidence_matches_cached_module(evidence_store, &cache, strict)? {
        return Ok(None);
    }
    let plan = OptimizationPlan::from_evidence(&cache.metadata, &cache.module, evidence_store);
    let output_path = module_optimization_plan_path(
        out_root,
        cache.metadata.source,
        cache.metadata.module_name.as_str(),
    )
    .with_context(|| {
        format!(
            "construct optimization plan output path for module {}",
            cache.metadata.module_name
        )
    })?;
    write_optimization_plan(output_path.as_path(), &plan)?;
    Ok(Some(ModuleOptimizationPlanReport {
        output_path,
        module_name: plan.module_name,
        source_hash: plan.source_hash,
        function_count: plan.functions.len(),
    }))
}

fn counter_evidence_matches_cached_module(
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

pub fn cached_module_paths_under_root(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_cached_module_paths(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_cached_module_paths(path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("read module cache path metadata {}", path.display()))?;
    if metadata.is_file() {
        if path.file_name().and_then(|name| name.to_str()) == Some("mod.blockpy") {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let entries = fs::read_dir(path)
        .with_context(|| format!("read module cache directory {}", path.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", path.display()))?;
        collect_cached_module_paths(entry.path().as_path(), out)?;
    }
    Ok(())
}

pub fn write_optimization_plan(path: &Path, plan: &OptimizationPlan) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create optimization plan dir {}", parent.display()))?;
    }
    let archive = rkyv::to_bytes::<rkyv::rancor::Error>(plan)
        .map_err(|err| anyhow::anyhow!("serialize optimization plan: {err}"))?;
    let temp_path = path.with_extension("opt.tmp");
    {
        let mut temp_file = File::create(temp_path.as_path()).with_context(|| {
            format!("create temporary optimization plan {}", temp_path.display())
        })?;
        temp_file
            .write_all(archive.as_ref())
            .with_context(|| format!("write optimization plan {}", temp_path.display()))?;
    }
    fs::rename(temp_path.as_path(), path).with_context(|| {
        format!(
            "publish optimization plan {} -> {}",
            temp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

fn validate_alternative_guard(
    alternative: &PlannedAlternative,
    predicate: impl Fn(&PlannedGuard) -> bool,
    context: &str,
) -> Result<()> {
    if alternative.guards.iter().any(predicate) {
        Ok(())
    } else {
        bail!("{context} is missing the matching guard")
    }
}

fn shape_map_for_family(
    evidence: &mut FunctionProfileEvidence,
    family: ShapeFamily,
) -> &mut HashMap<InstrId, Vec<u64>> {
    match family {
        ShapeFamily::Operator => &mut evidence.operator_specializations,
        ShapeFamily::GetItem => &mut evidence.getitem_specializations,
        ShapeFamily::SetItem => &mut evidence.setitem_specializations,
    }
}

fn decisions_from_evidence(
    evidence: &PersistentFunctionProfileEvidence,
    mut function_target_resolver: impl FnMut(PersistentFunctionId) -> PlannedFunctionTarget,
) -> Vec<OptimizationDecision> {
    let mut decisions = Vec::new();
    extend_call_target_decisions(
        &mut decisions,
        &evidence.call_target_specializations,
        &mut function_target_resolver,
    );
    extend_instr_u64_decisions(
        &mut decisions,
        &evidence.operator_specializations,
        U64DecisionKind::Operator,
    );
    extend_instr_u64_decisions(
        &mut decisions,
        &evidence.getitem_specializations,
        U64DecisionKind::GetItem,
    );
    extend_instr_u64_decisions(
        &mut decisions,
        &evidence.setitem_specializations,
        U64DecisionKind::SetItem,
    );
    extend_field_index_decisions(&mut decisions, &evidence.field_index_specializations);
    let mut branch_decisions = evidence
        .branch_prefer_true
        .iter()
        .map(|(instr_id, prefer_true)| OptimizationDecision {
            instr_id: *instr_id,
            replacement: PlannedReplacement::BranchPreference {
                prefer_true: *prefer_true,
            },
        })
        .collect::<Vec<_>>();
    branch_decisions.sort_by_key(decision_instr_id);
    decisions.extend(branch_decisions);
    decisions
        .sort_by_key(|decision| (decision_instr_id(decision), decision_variant_rank(decision)));
    decisions
}

fn extend_call_target_decisions(
    decisions: &mut Vec<OptimizationDecision>,
    values_by_instr: &HashMap<InstrId, Vec<PersistentFunctionId>>,
    function_target_resolver: &mut impl FnMut(PersistentFunctionId) -> PlannedFunctionTarget,
) {
    let mut entries = values_by_instr.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(instr_id, _values)| **instr_id);
    for (instr_id, values) in entries {
        let mut values = values.clone();
        values.sort();
        let alternatives = values
            .into_iter()
            .map(&mut *function_target_resolver)
            .map(|target| PlannedAlternative {
                guards: vec![PlannedGuard::FunctionTarget {
                    target: target.clone(),
                }],
                action: PlannedAction::DirectCall { target },
            })
            .collect::<Vec<_>>();
        if alternatives.is_empty() {
            continue;
        }
        decisions.push(OptimizationDecision {
            instr_id: *instr_id,
            replacement: guarded_replacement(alternatives),
        });
    }
}

fn extend_field_index_decisions(
    decisions: &mut Vec<OptimizationDecision>,
    values_by_instr: &HashMap<InstrId, Vec<PlannedIndexedFieldSpecialization>>,
) {
    let mut entries = values_by_instr.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(instr_id, _values)| **instr_id);
    for (instr_id, values) in entries {
        let mut values = values.clone();
        values.sort();
        decisions.push(OptimizationDecision {
            instr_id: *instr_id,
            replacement: guarded_replacement(
                values
                    .into_iter()
                    .map(|specialization| PlannedAlternative {
                        guards: vec![PlannedGuard::IndexedField {
                            specialization: specialization.clone(),
                        }],
                        action: PlannedAction::IndexedField { specialization },
                    })
                    .collect(),
            ),
        });
    }
}

fn extend_instr_u64_decisions(
    decisions: &mut Vec<OptimizationDecision>,
    values_by_instr: &HashMap<InstrId, Vec<u64>>,
    kind: U64DecisionKind,
) {
    let mut entries = values_by_instr.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(instr_id, _values)| **instr_id);
    for (instr_id, values) in entries {
        let mut values = values.clone();
        values.sort();
        let family = kind.shape_family();
        decisions.push(OptimizationDecision {
            instr_id: *instr_id,
            replacement: guarded_replacement(
                values
                    .into_iter()
                    .map(|shape| PlannedAlternative {
                        guards: vec![PlannedGuard::ObservedShape { family, shape }],
                        action: PlannedAction::SpecializedShape { family, shape },
                    })
                    .collect(),
            ),
        });
    }
}

#[derive(Clone, Copy)]
enum U64DecisionKind {
    Operator,
    GetItem,
    SetItem,
}

impl U64DecisionKind {
    const fn shape_family(self) -> ShapeFamily {
        match self {
            Self::Operator => ShapeFamily::Operator,
            Self::GetItem => ShapeFamily::GetItem,
            Self::SetItem => ShapeFamily::SetItem,
        }
    }
}

fn guarded_replacement(alternatives: Vec<PlannedAlternative>) -> PlannedReplacement {
    PlannedReplacement::Guarded {
        alternatives,
        fallback: PlannedFallback::OriginalInstruction,
    }
}

fn decision_variant_rank(decision: &OptimizationDecision) -> u8 {
    match &decision.replacement {
        PlannedReplacement::Guarded {
            alternatives,
            fallback: _,
        } => alternatives
            .first()
            .map(|alternative| match alternative.action {
                PlannedAction::DirectCall { .. } => 0,
                PlannedAction::SpecializedShape {
                    family: ShapeFamily::Operator,
                    ..
                } => 1,
                PlannedAction::SpecializedShape {
                    family: ShapeFamily::GetItem,
                    ..
                } => 2,
                PlannedAction::SpecializedShape {
                    family: ShapeFamily::SetItem,
                    ..
                } => 3,
                PlannedAction::IndexedField { .. } => 4,
            })
            .unwrap_or(5),
        PlannedReplacement::BranchPreference { .. } => 6,
    }
}

fn decisions_for_function(
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
    evidence_store: &ProfileEvidenceStore,
    evidence: &PersistentFunctionProfileEvidence,
    function_target_resolver: impl FnMut(PersistentFunctionId) -> PlannedFunctionTarget,
) -> Vec<OptimizationDecision> {
    let mut evidence = evidence.clone();
    add_field_index_evidence_for_function(module, function, evidence_store, &mut evidence);
    decisions_from_evidence(&evidence, function_target_resolver)
}

fn add_field_index_evidence_for_function(
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
    evidence_store: &ProfileEvidenceStore,
    evidence: &mut PersistentFunctionProfileEvidence,
) {
    struct Collector<'a> {
        module: &'a BlockPyModule<CodegenModuleShape>,
        evidence_store: &'a ProfileEvidenceStore,
        evidence: &'a mut PersistentFunctionProfileEvidence,
    }

    impl Collector<'_> {
        fn collect_attr(&mut self, instr_id: InstrId, attr_expr: &InstrCodegen) {
            let Some(attr_name) = codegen_constant_string_value(self.module, attr_expr) else {
                return;
            };
            let Some(specializations) = self
                .evidence_store
                .field_index_specializations_for_attr(attr_name)
            else {
                return;
            };
            for specialization in specializations {
                push_unique(
                    self.evidence
                        .field_index_specializations
                        .entry(instr_id)
                        .or_default(),
                    specialization.clone(),
                );
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
                    self.collect_attr(op.semantic_instr_id(), op.attr.as_ref());
                }
                InstrCodegen::SetAttr(op) => {
                    self.collect_attr(op.semantic_instr_id(), op.attr.as_ref());
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        module,
        evidence_store,
        evidence,
    };
    collector.visit_fn(function);
}

fn module_constant_string_value<'a>(
    module: &'a BlockPyModule<CodegenModuleShape>,
    constant_index: u32,
) -> Option<&'a str> {
    let InstrResolved::Literal(literal) = module.module_constants.get(constant_index as usize)?
    else {
        return None;
    };
    let Literal::StringLiteral(literal) = literal.as_literal() else {
        return None;
    };
    Some(literal.value.as_str())
}

fn codegen_constant_string_value<'a>(
    module: &'a BlockPyModule<CodegenModuleShape>,
    expr: &InstrCodegen,
) -> Option<&'a str> {
    let InstrCodegen::Load(load) = expr else {
        return None;
    };
    let NameLocation::Constant(constant_index) = load.name.location else {
        return None;
    };
    module_constant_string_value(module, constant_index)
}

fn decision_instr_id(decision: &OptimizationDecision) -> InstrId {
    decision.instr_id
}

pub fn format_optimization_plan(plan: &OptimizationPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "module {} source={:?} source_hash=0x{:016x} cache_identity={}\n",
        plan.module_name, plan.source, plan.source_hash, plan.cache_identity
    ));
    for function in &plan.functions {
        out.push_str(&format!(
            "function {} {}\n",
            function.function,
            plan.debug_name_for_function(function.function)
                .unwrap_or("<unknown>")
        ));
        for decision in &function.decisions {
            out.push_str("  ");
            out.push_str(&format_decision(plan, decision));
            out.push('\n');
        }
    }
    out
}

fn format_decision(plan: &OptimizationPlan, decision: &OptimizationDecision) -> String {
    match &decision.replacement {
        PlannedReplacement::Guarded {
            alternatives,
            fallback,
        } => {
            let alternatives = alternatives
                .iter()
                .map(|alternative| format_alternative(plan, alternative))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "instr_id={} => Guard([{}], {})",
                decision.instr_id,
                alternatives,
                format_fallback(fallback)
            )
        }
        PlannedReplacement::BranchPreference { prefer_true } => {
            format!(
                "instr_id={} => BranchPreference(prefer_true={prefer_true})",
                decision.instr_id
            )
        }
    }
}

fn format_alternative(plan: &OptimizationPlan, alternative: &PlannedAlternative) -> String {
    let guards = alternative
        .guards
        .iter()
        .map(|guard| format_guard(plan, guard))
        .collect::<Vec<_>>()
        .join(" && ");
    format!("({guards}) => {}", format_action(plan, &alternative.action))
}

fn format_guard(plan: &OptimizationPlan, guard: &PlannedGuard) -> String {
    match guard {
        PlannedGuard::FunctionTarget { target } => {
            format!("FunctionTarget({})", format_function_target(plan, target))
        }
        PlannedGuard::ObservedShape { family, shape } => {
            format!("{}Shape({shape})", format_shape_family(*family))
        }
        PlannedGuard::IndexedField { specialization } => {
            format!("IndexedField({})", format_indexed_field(specialization))
        }
    }
}

fn format_action(plan: &OptimizationPlan, action: &PlannedAction) -> String {
    match action {
        PlannedAction::DirectCall { target } => {
            format!("DirectCall({})", format_function_target(plan, target))
        }
        PlannedAction::SpecializedShape { family, shape } => {
            format!("Specialized{}({shape})", format_shape_family(*family))
        }
        PlannedAction::IndexedField { specialization } => {
            format!("IndexedField({})", format_indexed_field(specialization))
        }
    }
}

fn format_function_target(plan: &OptimizationPlan, target: &PlannedFunctionTarget) -> String {
    match plan.persistent_function_id(target.function) {
        Ok(function) => format!(
            "{}:{}#0x{:016x}",
            function.module.module_name, function.local, function.module.source_hash
        ),
        Err(_) => target.function.to_string(),
    }
}

fn format_indexed_field(specialization: &PlannedIndexedFieldSpecialization) -> String {
    format!(
        "{}.{} attr={} index={}",
        specialization.owner_type.module_name,
        specialization.owner_type.qualname,
        specialization.attr_name,
        specialization.expected_index
    )
}

fn format_fallback(fallback: &PlannedFallback) -> &'static str {
    match fallback {
        PlannedFallback::OriginalInstruction => "OriginalInstruction",
    }
}

fn format_shape_family(family: ShapeFamily) -> &'static str {
    match family {
        ShapeFamily::Operator => "Operator",
        ShapeFamily::GetItem => "GetItem",
        ShapeFamily::SetItem => "SetItem",
    }
}

fn push_observed_shape(
    shapes_by_instr: &mut HashMap<InstrId, Vec<u64>>,
    instr_id: InstrId,
    observed_value: Option<u64>,
) {
    let Some(observed_value) = observed_value else {
        return;
    };
    if observed_value == 0 {
        return;
    }
    push_unique(shapes_by_instr.entry(instr_id).or_default(), observed_value);
}

fn push_unique<T: Eq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::counter_dump::{
        CounterDumpRecord, CounterDumpRow, CounterDumpTypeKey, CounterDumpTypeKeyLayout,
        CounterDumpTypeTableEntry,
    };
    use soac_core::block_py::{BlockLabel, InstrId};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn planned_function_target_roundtrips_persistent_identity_without_qualname() {
        let persistent = PersistentFunctionId::new(
            ModuleContentId::new("pkg.mod", 0x1234),
            LocalFunctionId::new(7),
        );
        let target = PlannedFunctionTarget::new(SerializedFunctionId::new(
            SerializedModuleId::new(0),
            persistent.local,
        ));
        let plan = test_plan_with_module("pkg.mod", 0x1234, target.function);

        assert_eq!(target.local_function_id(), LocalFunctionId::new(7));
        assert_eq!(
            plan.persistent_function_id(target.function).unwrap(),
            persistent
        );
    }

    #[test]
    fn profile_evidence_store_loads_counter_dump_once_into_function_views() {
        let function_id = RuntimeFunctionId::from_raw_parts(7, 1);
        let instr_id = InstrId::new(BlockLabel::from_index(3), 4);
        let target_id = RuntimeFunctionId::from_raw_parts(7, 2);
        let target_persistent = PersistentFunctionId::new(
            ModuleContentId::new("pkg.mod", 0x1234),
            target_id.local_function_id(),
        );
        let target = PlannedFunctionTarget::new(SerializedFunctionId::new(
            SerializedModuleId::new(0),
            target_id.local_function_id(),
        ));
        let field_specialization = PlannedIndexedFieldSpecialization {
            owner_type: PlannedTypeKey {
                module_name: "pkg.types".to_string(),
                qualname: "Point".to_string(),
            },
            attr_name: "x".to_string(),
            expected_index: 2,
        };
        let rows = vec![
            row(
                "call_hot_targets",
                function_id,
                instr_id,
                1,
                Some(target_id.to_packed_runtime_u64()),
            ),
            row(
                "call_hot_targets",
                function_id,
                instr_id,
                1,
                Some(target_id.to_packed_runtime_u64()),
            ),
            row("operator_hot_shapes", function_id, instr_id, 1, Some(257)),
            row("branch_outcomes", function_id, instr_id, 2, Some(1)),
            row("branch_outcomes", function_id, instr_id, 1, Some(0)),
        ];
        let record = CounterDumpRecord {
            source_hash: 0x1234,
            module_name: "pkg.mod".to_string(),
            package_name: None,
            rows,
            module_keys: Vec::new(),
            type_keys: vec![CounterDumpTypeKeyLayout {
                owner_type_id: 44,
                key: "x".to_string(),
                index: 2,
            }],
            type_table: vec![CounterDumpTypeTableEntry {
                type_id: 44,
                key: CounterDumpTypeKey {
                    module_name: "pkg.types".to_string(),
                    qualname: "Point".to_string(),
                },
            }],
        };
        let path = unique_counter_path();
        fs::write(path.as_path(), record.encode().unwrap()).unwrap();

        let store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let evidence = store.for_function("pkg.mod", 0x1234, function_id);
        let _ = fs::remove_file(path);

        assert_eq!(store.module_source_hash("pkg.mod"), Some(0x1234));
        assert_eq!(
            evidence.call_target_specializations.get(&instr_id).unwrap(),
            &vec![target_persistent.clone()]
        );
        assert_eq!(
            evidence.operator_specializations.get(&instr_id).unwrap(),
            &vec![257]
        );
        assert_eq!(evidence.branch_prefer_true.get(&instr_id), Some(&true));
        assert_eq!(
            store.field_index_specializations_for_attr("x").unwrap(),
            &[field_specialization.clone()]
        );

        let mut evidence = evidence;
        evidence
            .field_index_specializations
            .insert(instr_id, vec![field_specialization.clone()]);
        let decisions = decisions_from_evidence(&evidence, |function_id| {
            assert_eq!(function_id, target_persistent);
            target.clone()
        });
        assert!(matches!(
            decisions[0].replacement,
            PlannedReplacement::Guarded { .. }
        ));
        assert_eq!(decisions[0].instr_id, instr_id);
        let PlannedReplacement::Guarded { alternatives, .. } = &decisions[0].replacement else {
            unreachable!("first decision is checked as guarded above");
        };
        assert_eq!(
            alternatives[0].action,
            PlannedAction::DirectCall {
                target: target.clone()
            }
        );
        assert_eq!(
            decisions.last().map(|decision| &decision.replacement),
            Some(&PlannedReplacement::BranchPreference { prefer_true: true })
        );

        let plan = OptimizationPlan {
            source: PythonModuleCacheSource::Project,
            module_name: "pkg.mod".to_string(),
            source_hash: 0x1234,
            cache_identity: "test-cache".to_string(),
            identity_tables: SerializedIdentityTables {
                modules: vec![SerializedModuleIdentity {
                    module_name: "pkg.mod".to_string(),
                    source_hash: 0x1234,
                    cache_identity: Some("test-cache".to_string()),
                }],
                debug_names: vec![SerializedFunctionDebugName {
                    function: SerializedFunctionId::new(
                        SerializedModuleId::new(0),
                        function_id.local_function_id(),
                    ),
                    qualname: "f".to_string(),
                }],
            },
            functions: vec![FunctionOptimizationPlan {
                function: SerializedFunctionId::new(
                    SerializedModuleId::new(0),
                    function_id.local_function_id(),
                ),
                decisions,
            }],
        };
        plan.validate_for_module(
            Some(PythonModuleCacheSource::Project),
            "pkg.mod",
            0x1234,
            "test-cache",
        )
        .unwrap();
        let planned_evidence = plan
            .evidence_by_local_function(|planned_target| {
                Ok((planned_target == &target_persistent).then_some(target_id))
            })
            .unwrap();
        let planned_function = planned_evidence
            .get(&function_id.local_function_id())
            .unwrap();
        assert_eq!(
            planned_function
                .call_target_specializations
                .get(&instr_id)
                .unwrap(),
            &vec![target_id]
        );
        assert_eq!(
            planned_function
                .operator_specializations
                .get(&instr_id)
                .unwrap(),
            &vec![257]
        );
        assert_eq!(
            planned_function
                .field_index_specializations
                .get(&instr_id)
                .unwrap(),
            &vec![field_specialization]
        );
        assert_eq!(
            planned_function.branch_prefer_true.get(&instr_id),
            Some(&true)
        );
    }

    #[test]
    fn profile_evidence_store_synthesizes_targets_from_loaded_module_identity() {
        let caller_id = RuntimeFunctionId::from_raw_parts(7, 1);
        let target_id = RuntimeFunctionId::from_raw_parts(8, 2);
        let unrelated_callee_id = RuntimeFunctionId::from_raw_parts(8, 99);
        let instr_id = InstrId::new(BlockLabel::from_index(3), 4);
        let caller_record = CounterDumpRecord {
            source_hash: 0x1234,
            module_name: "pkg.caller".to_string(),
            package_name: None,
            rows: vec![row(
                "call_hot_targets",
                caller_id,
                instr_id,
                1,
                Some(target_id.to_packed_runtime_u64()),
            )],
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        };
        let callee_record = CounterDumpRecord {
            source_hash: 0x5678,
            module_name: "pkg.callee".to_string(),
            package_name: None,
            rows: vec![row(
                "operator_hot_shapes",
                unrelated_callee_id,
                instr_id,
                1,
                Some(257),
            )],
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        };
        let path = unique_counter_path();
        let mut bytes = caller_record.encode().unwrap();
        bytes.extend_from_slice(callee_record.encode().unwrap().as_slice());
        fs::write(path.as_path(), bytes).unwrap();

        let store = ProfileEvidenceStore::from_counter_dump(path.as_path()).unwrap();
        let _ = fs::remove_file(path);
        let synthesized = store
            .function_target(target_id)
            .expect("known module id should synthesize target metadata");
        assert_eq!(
            synthesized,
            PersistentFunctionId::new(
                ModuleContentId::new("pkg.callee", 0x5678),
                target_id.local_function_id()
            )
        );
        let serialized_target = PlannedFunctionTarget::new(SerializedFunctionId::new(
            SerializedModuleId::new(0),
            target_id.local_function_id(),
        ));

        let decisions =
            decisions_from_evidence(&store.for_function("pkg.caller", 0x1234, caller_id), |id| {
                assert_eq!(id, synthesized);
                serialized_target.clone()
            });
        let PlannedReplacement::Guarded { alternatives, .. } = &decisions[0].replacement else {
            unreachable!("call target decision should be guarded");
        };
        assert_eq!(
            alternatives[0].action,
            PlannedAction::DirectCall {
                target: serialized_target,
            }
        );
    }

    fn test_plan_with_module(
        module_name: &str,
        source_hash: u64,
        function: SerializedFunctionId,
    ) -> OptimizationPlan {
        OptimizationPlan {
            source: PythonModuleCacheSource::Project,
            module_name: module_name.to_string(),
            source_hash,
            cache_identity: "test-cache".to_string(),
            identity_tables: SerializedIdentityTables {
                modules: vec![SerializedModuleIdentity {
                    module_name: module_name.to_string(),
                    source_hash,
                    cache_identity: Some("test-cache".to_string()),
                }],
                debug_names: vec![SerializedFunctionDebugName {
                    function,
                    qualname: "f".to_string(),
                }],
            },
            functions: Vec::new(),
        }
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
            observed_value,
            max_overcount: None,
        }
    }

    fn unique_counter_path() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac-profile-evidence-store-test-{}-{unique}.bin",
            std::process::id()
        ))
    }
}
