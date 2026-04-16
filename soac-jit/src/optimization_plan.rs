use crate::counter_dump::CounterDumpFile;
use anyhow::{Context, Result, bail};
use soac_blockpy::block_py::{BlockPyModule, FunctionId, InstrId};
use soac_blockpy::codegen_cache::{CachedCodegenModuleMetadata, PythonModuleCacheSource};
use soac_blockpy::passes::CodegenModuleShape;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionProfileEvidence {
    pub call_target_specializations: HashMap<InstrId, Vec<FunctionId>>,
    pub operator_specializations: HashMap<InstrId, Vec<u64>>,
    pub getitem_specializations: HashMap<InstrId, Vec<u64>>,
    pub setitem_specializations: HashMap<InstrId, Vec<u64>>,
    pub branch_prefer_true: HashMap<InstrId, bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ProfileEvidenceStore {
    functions: HashMap<(String, FunctionId), FunctionProfileEvidence>,
    module_source_hashes: HashMap<String, u64>,
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
    FunctionId { function_id: FunctionId },
    ObservedShape { family: ShapeFamily, shape: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PlannedAction {
    DirectCall { function_id: FunctionId },
    SpecializedShape { family: ShapeFamily, shape: u64 },
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

        for record in records {
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
}

impl OptimizationPlan {
    pub fn from_evidence(
        metadata: &CachedCodegenModuleMetadata,
        module: &BlockPyModule<CodegenModuleShape>,
        evidence_store: &ProfileEvidenceStore,
    ) -> Self {
        let mut functions = module
            .callable_defs
            .iter()
            .filter_map(|function| {
                let evidence = evidence_store
                    .for_function(metadata.module_name.as_str(), function.function_id);
                let decisions = decisions_from_evidence(&evidence);
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
}

fn decisions_from_evidence(evidence: &FunctionProfileEvidence) -> Vec<OptimizationDecision> {
    let mut decisions = Vec::new();
    extend_call_target_decisions(&mut decisions, &evidence.call_target_specializations);
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
                    .map(|function_id| PlannedAlternative {
                        guards: vec![PlannedGuard::FunctionId { function_id }],
                        action: PlannedAction::DirectCall { function_id },
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
            })
            .unwrap_or(4),
        PlannedReplacement::BranchPreference { .. } => 5,
    }
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
        PlannedGuard::FunctionId { function_id } => format!("FunctionId({function_id})"),
        PlannedGuard::ObservedShape { family, shape } => {
            format!("{}Shape({shape})", format_shape_family(*family))
        }
    }
}

fn format_action(action: &PlannedAction) -> String {
    match action {
        PlannedAction::DirectCall { function_id } => format!("DirectCall({function_id})"),
        PlannedAction::SpecializedShape { family, shape } => {
            format!("Specialized{}({shape})", format_shape_family(*family))
        }
    }
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

fn push_unique<T: Copy + Eq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::counter_dump::{CounterDumpRecord, CounterDumpRow};
    use soac_blockpy::block_py::{BlockLabel, InstrId};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn profile_evidence_store_loads_counter_dump_once_into_function_views() {
        let function_id = FunctionId::new(7, 1);
        let instr_id = InstrId::new(BlockLabel::from_index(3), 4);
        let target_id = FunctionId::new(7, 2);
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
            type_keys: Vec::new(),
            type_table: Vec::new(),
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

        let decisions = decisions_from_evidence(&evidence);
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
                function_id: target_id
            }
        );
        assert_eq!(
            decisions.last().map(|decision| &decision.replacement),
            Some(&PlannedReplacement::BranchPreference { prefer_true: true })
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
