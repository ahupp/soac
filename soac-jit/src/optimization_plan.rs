use crate::counter_dump::{CounterDumpFile, collect_type_key_layouts, collect_type_table};
use anyhow::{Context, Result, bail};
use soac_blockpy::block_py::{
    BlockPyFunction, BlockPyModule, ChildVisitable, FunctionId, HasSemanticInstrId, InstrCodegen,
    InstrId, Literal, NameLocation, Visit,
};
use soac_blockpy::codegen_cache::{CachedCodegenModuleMetadata, PythonModuleCacheSource};
use soac_blockpy::passes::{CodegenModuleShape, InstrResolved};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionProfileEvidence {
    pub call_target_specializations: HashMap<InstrId, Vec<FunctionId>>,
    pub operator_specializations: HashMap<InstrId, Vec<u64>>,
    pub getitem_specializations: HashMap<InstrId, Vec<u64>>,
    pub setitem_specializations: HashMap<InstrId, Vec<u64>>,
    pub field_index_specializations: HashMap<InstrId, Vec<PlannedIndexedFieldSpecialization>>,
    pub branch_prefer_true: HashMap<InstrId, bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ProfileEvidenceStore {
    functions: HashMap<(String, FunctionId), FunctionProfileEvidence>,
    module_source_hashes: HashMap<String, u64>,
    function_targets: HashMap<FunctionId, PlannedFunctionTarget>,
    field_index_specializations_by_attr: HashMap<String, Vec<PlannedIndexedFieldSpecialization>>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct OptimizationPlan {
    pub source: PythonModuleCacheSource,
    pub module_name: String,
    pub source_hash: u64,
    pub cache_identity: String,
    pub functions: Vec<FunctionOptimizationPlan>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FunctionOptimizationPlan {
    pub function_id: FunctionId,
    pub qualname: String,
    pub decisions: Vec<OptimizationDecision>,
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
    pub module_name: String,
    pub source_hash: u64,
    pub function_id: u32,
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
        let mut branch_counts = HashMap::<(String, FunctionId, InstrId), [u64; 2]>::new();

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
                store
                    .function_targets
                    .entry(function_id)
                    .or_insert_with(|| PlannedFunctionTarget {
                        module_name: module_name.clone(),
                        source_hash: record.source_hash(),
                        function_id: function_id.function_id(),
                    });
                if let Some(current_function_id) = row.current_function_id {
                    store
                        .function_targets
                        .entry(current_function_id)
                        .or_insert_with(|| PlannedFunctionTarget {
                            module_name: module_name.clone(),
                            source_hash: record.source_hash(),
                            function_id: current_function_id.function_id(),
                        });
                }
                let Some(instr_id) = row.instr_id else {
                    continue;
                };
                let function = store
                    .functions
                    .entry((module_name.clone(), function_id))
                    .or_default();
                match row.kind {
                    "call_hot_targets" => {
                        let Some(observed_value) = row.observed_value else {
                            continue;
                        };
                        if observed_value == 0 {
                            continue;
                        }
                        let observed = FunctionId::from_packed(observed_value);
                        if observed == FunctionId::global() {
                            continue;
                        }
                        push_unique(
                            function
                                .call_target_specializations
                                .entry(instr_id)
                                .or_default(),
                            observed,
                        );
                    }
                    "operator_hot_shapes" => push_observed_shape(
                        &mut function.operator_specializations,
                        instr_id,
                        row.observed_value,
                    ),
                    "getitem_hot_shapes" => push_observed_shape(
                        &mut function.getitem_specializations,
                        instr_id,
                        row.observed_value,
                    ),
                    "setitem_hot_shapes" => push_observed_shape(
                        &mut function.setitem_specializations,
                        instr_id,
                        row.observed_value,
                    ),
                    "branch_outcomes" => {
                        let Some(slot) = row
                            .observed_value
                            .and_then(|value| usize::try_from(value).ok())
                        else {
                            continue;
                        };
                        if slot < 2 {
                            let counts = branch_counts
                                .entry((module_name.clone(), function_id, instr_id))
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

        for ((module_name, function_id, instr_id), [false_count, true_count]) in branch_counts {
            if false_count == 0 && true_count == 0 {
                continue;
            }
            store
                .functions
                .entry((module_name, function_id))
                .or_default()
                .branch_prefer_true
                .insert(instr_id, true_count >= false_count);
        }

        Ok(store)
    }

    pub fn for_function(
        &self,
        module_name: &str,
        function_id: FunctionId,
    ) -> FunctionProfileEvidence {
        self.functions
            .get(&(module_name.to_string(), function_id))
            .cloned()
            .unwrap_or_default()
    }

    pub fn module_source_hash(&self, module_name: &str) -> Option<u64> {
        self.module_source_hashes.get(module_name).copied()
    }

    pub fn function_target(&self, function_id: FunctionId) -> Option<&PlannedFunctionTarget> {
        self.function_targets.get(&function_id)
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

impl OptimizationPlan {
    pub fn from_evidence(
        metadata: &CachedCodegenModuleMetadata,
        module: &BlockPyModule<CodegenModuleShape>,
        evidence_store: &ProfileEvidenceStore,
    ) -> Self {
        let local_function_targets = module
            .callable_defs
            .iter()
            .map(|function| {
                (
                    function.function_id,
                    PlannedFunctionTarget {
                        module_name: metadata.module_name.clone(),
                        source_hash: metadata.source_hash,
                        function_id: function.function_id.function_id(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let mut functions = module
            .callable_defs
            .iter()
            .filter_map(|function| {
                let evidence = evidence_store
                    .for_function(metadata.module_name.as_str(), function.function_id);
                let decisions = decisions_for_function(
                    module,
                    function,
                    evidence_store,
                    &local_function_targets,
                    &evidence,
                );
                (!decisions.is_empty()).then(|| FunctionOptimizationPlan {
                    function_id: function.function_id,
                    qualname: function.names.qualname.clone(),
                    decisions,
                })
            })
            .collect::<Vec<_>>();
        functions.sort_by_key(|function| function.function_id);
        Self {
            source: metadata.source,
            module_name: metadata.module_name.clone(),
            source_hash: metadata.source_hash,
            cache_identity: metadata.cache_identity.clone(),
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
        Ok(())
    }

    pub fn evidence_for_function(
        &self,
        function_id: FunctionId,
        call_target_resolver: impl Fn(&PlannedFunctionTarget) -> Result<Option<FunctionId>>,
    ) -> Result<FunctionProfileEvidence> {
        let Some(function) = self
            .functions
            .iter()
            .find(|function| function.function_id == function_id)
        else {
            return Ok(FunctionProfileEvidence::default());
        };
        let mut evidence = FunctionProfileEvidence::default();
        for decision in &function.decisions {
            apply_decision_to_evidence(decision, &mut evidence, &call_target_resolver)?;
        }
        Ok(evidence)
    }

    pub fn evidence_by_function(
        &self,
        call_target_resolver: impl Fn(&PlannedFunctionTarget) -> Result<Option<FunctionId>>,
    ) -> Result<HashMap<FunctionId, FunctionProfileEvidence>> {
        let mut out = HashMap::new();
        for function in &self.functions {
            let mut evidence = FunctionProfileEvidence::default();
            for decision in &function.decisions {
                apply_decision_to_evidence(decision, &mut evidence, &call_target_resolver)?;
            }
            out.insert(function.function_id, evidence);
        }
        Ok(out)
    }
}

pub fn load_optimization_plan(path: &Path) -> Result<OptimizationPlan> {
    let bytes =
        fs::read(path).with_context(|| format!("read optimization plan {}", path.display()))?;
    rkyv::from_bytes::<OptimizationPlan, rkyv::rancor::Error>(bytes.as_slice())
        .map_err(|err| anyhow::anyhow!("deserialize optimization plan {}: {err}", path.display()))
}

fn apply_decision_to_evidence(
    decision: &OptimizationDecision,
    evidence: &mut FunctionProfileEvidence,
    call_target_resolver: &impl Fn(&PlannedFunctionTarget) -> Result<Option<FunctionId>>,
) -> Result<()> {
    match &decision.replacement {
        PlannedReplacement::Guarded {
            alternatives,
            fallback: PlannedFallback::OriginalInstruction,
        } => {
            for alternative in alternatives {
                apply_alternative_to_evidence(
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
    instr_id: InstrId,
    alternative: &PlannedAlternative,
    evidence: &mut FunctionProfileEvidence,
    call_target_resolver: &impl Fn(&PlannedFunctionTarget) -> Result<Option<FunctionId>>,
) -> Result<()> {
    match &alternative.action {
        PlannedAction::DirectCall { target } => {
            validate_alternative_guard(
                alternative,
                |guard| matches!(guard, PlannedGuard::FunctionTarget { target: guarded } if guarded == target),
                "direct-call alternative",
            )?;
            if let Some(function_id) = call_target_resolver(target)? {
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
    evidence: &FunctionProfileEvidence,
    function_target_resolver: impl Fn(FunctionId) -> Option<PlannedFunctionTarget>,
) -> Vec<OptimizationDecision> {
    let mut decisions = Vec::new();
    extend_call_target_decisions(
        &mut decisions,
        &evidence.call_target_specializations,
        &function_target_resolver,
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
    values_by_instr: &HashMap<InstrId, Vec<FunctionId>>,
    function_target_resolver: &impl Fn(FunctionId) -> Option<PlannedFunctionTarget>,
) {
    let mut entries = values_by_instr.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(instr_id, _values)| **instr_id);
    for (instr_id, values) in entries {
        let mut values = values.clone();
        values.sort();
        let alternatives = values
            .into_iter()
            .filter_map(function_target_resolver)
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
    local_function_targets: &HashMap<FunctionId, PlannedFunctionTarget>,
    evidence: &FunctionProfileEvidence,
) -> Vec<OptimizationDecision> {
    let mut evidence = evidence.clone();
    add_field_index_evidence_for_function(module, function, evidence_store, &mut evidence);
    decisions_from_evidence(&evidence, |function_id| {
        local_function_targets
            .get(&function_id)
            .or_else(|| evidence_store.function_target(function_id))
            .cloned()
    })
}

fn add_field_index_evidence_for_function(
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
    evidence_store: &ProfileEvidenceStore,
    evidence: &mut FunctionProfileEvidence,
) {
    struct Collector<'a> {
        module: &'a BlockPyModule<CodegenModuleShape>,
        evidence_store: &'a ProfileEvidenceStore,
        evidence: &'a mut FunctionProfileEvidence,
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
            function.function_id, function.qualname
        ));
        for decision in &function.decisions {
            out.push_str("  ");
            out.push_str(&format_decision(decision));
            out.push('\n');
        }
    }
    out
}

fn format_decision(decision: &OptimizationDecision) -> String {
    match &decision.replacement {
        PlannedReplacement::Guarded {
            alternatives,
            fallback,
        } => {
            let alternatives = alternatives
                .iter()
                .map(format_alternative)
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

fn format_alternative(alternative: &PlannedAlternative) -> String {
    let guards = alternative
        .guards
        .iter()
        .map(format_guard)
        .collect::<Vec<_>>()
        .join(" && ");
    format!("({guards}) => {}", format_action(&alternative.action))
}

fn format_guard(guard: &PlannedGuard) -> String {
    match guard {
        PlannedGuard::FunctionTarget { target } => {
            format!("FunctionTarget({})", format_function_target(target))
        }
        PlannedGuard::ObservedShape { family, shape } => {
            format!("{}Shape({shape})", format_shape_family(*family))
        }
        PlannedGuard::IndexedField { specialization } => {
            format!("IndexedField({})", format_indexed_field(specialization))
        }
    }
}

fn format_action(action: &PlannedAction) -> String {
    match action {
        PlannedAction::DirectCall { target } => {
            format!("DirectCall({})", format_function_target(target))
        }
        PlannedAction::SpecializedShape { family, shape } => {
            format!("Specialized{}({shape})", format_shape_family(*family))
        }
        PlannedAction::IndexedField { specialization } => {
            format!("IndexedField({})", format_indexed_field(specialization))
        }
    }
}

fn format_function_target(target: &PlannedFunctionTarget) -> String {
    format!(
        "{}:{}#0x{:016x}",
        target.module_name, target.function_id, target.source_hash
    )
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
    use soac_blockpy::block_py::{BlockLabel, InstrId};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn profile_evidence_store_loads_counter_dump_once_into_function_views() {
        let function_id = FunctionId::new(7, 1);
        let instr_id = InstrId::new(BlockLabel::from_index(3), 4);
        let target_id = FunctionId::new(7, 2);
        let target = PlannedFunctionTarget {
            module_name: "pkg.mod".to_string(),
            source_hash: 0x1234,
            function_id: target_id.function_id(),
        };
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
                Some(target_id.packed()),
            ),
            row(
                "call_hot_targets",
                function_id,
                instr_id,
                1,
                Some(target_id.packed()),
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
        let evidence = store.for_function("pkg.mod", function_id);
        let _ = fs::remove_file(path);

        assert_eq!(store.module_source_hash("pkg.mod"), Some(0x1234));
        assert_eq!(
            evidence.call_target_specializations.get(&instr_id).unwrap(),
            &vec![target_id]
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
            (function_id == target_id).then(|| target.clone())
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
            functions: vec![FunctionOptimizationPlan {
                function_id,
                qualname: "f".to_string(),
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
            .evidence_by_function(|planned_target| {
                Ok((planned_target == &target).then_some(target_id))
            })
            .unwrap();
        let planned_function = planned_evidence.get(&function_id).unwrap();
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

    fn row(
        kind: &str,
        function_id: FunctionId,
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
