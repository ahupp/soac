use anyhow::{Context, Result, bail};
use soac_core::block_py::{
    InstrId, ModuleContentId, PersistentFunctionId, RuntimeFunctionId, RuntimeModuleId,
};
use soac_core::profile::{
    CounterDumpFile, collect_module_key_layouts, collect_type_key_layouts, collect_type_table,
};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionProfileEvidence {
    pub call_target_specializations: HashMap<InstrId, Vec<RuntimeFunctionId>>,
    pub operator_specializations: HashMap<InstrId, Vec<u64>>,
    pub getitem_specializations: HashMap<InstrId, Vec<u64>>,
    pub setitem_specializations: HashMap<InstrId, Vec<u64>>,
    pub branch_prefer_true: HashMap<InstrId, bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PersistentFunctionProfileEvidence {
    call_target_specializations: HashMap<InstrId, Vec<PersistentFunctionId>>,
    operator_specializations: HashMap<InstrId, Vec<u64>>,
    getitem_specializations: HashMap<InstrId, Vec<u64>>,
    setitem_specializations: HashMap<InstrId, Vec<u64>>,
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
    global_index_specializations_by_module_name:
        HashMap<(String, String), Vec<PlannedIndexedGlobalSpecialization>>,
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
pub struct PlannedIndexedGlobalSpecialization {
    pub module_name: String,
    pub name: String,
    pub expected_index: u32,
}

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct PlannedTypeKey {
    pub module_name: String,
    pub qualname: String,
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
        let module_key_layouts =
            collect_module_key_layouts(records.as_slice()).map_err(anyhow::Error::msg)?;
        for (module_name, layouts) in module_key_layouts {
            for layout in layouts {
                let specialization = PlannedIndexedGlobalSpecialization {
                    module_name: module_name.clone(),
                    name: layout.key,
                    expected_index: layout.index,
                };
                push_unique(
                    store
                        .global_index_specializations_by_module_name
                        .entry((
                            specialization.module_name.clone(),
                            specialization.name.clone(),
                        ))
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

    pub fn global_index_specializations_for_name(
        &self,
        module_name: &str,
        name: &str,
    ) -> Option<&[PlannedIndexedGlobalSpecialization]> {
        self.global_index_specializations_by_module_name
            .get(&(module_name.to_string(), name.to_string()))
            .map(Vec::as_slice)
    }

    pub fn evidence_for_runtime_function_v3(
        &self,
        module_name: &str,
        source_hash: u64,
        function_id: RuntimeFunctionId,
    ) -> FunctionProfileEvidence {
        let persistent = self.for_function(module_name, source_hash, function_id);
        let mut call_target_specializations = HashMap::new();
        for (instr_id, targets) in persistent.call_target_specializations {
            for target in targets {
                if target.module.module_name != module_name
                    || target.module.source_hash != source_hash
                {
                    continue;
                }
                push_unique(
                    call_target_specializations.entry(instr_id).or_default(),
                    RuntimeFunctionId::new(function_id.runtime_module_id(), target.local),
                );
            }
        }
        FunctionProfileEvidence {
            call_target_specializations,
            operator_specializations: persistent.operator_specializations,
            getitem_specializations: persistent.getitem_specializations,
            setitem_specializations: persistent.setitem_specializations,
            branch_prefer_true: persistent.branch_prefer_true,
        }
    }

    pub fn persistent_call_target_specializations_for_runtime_function_v3(
        &self,
        module_name: &str,
        source_hash: u64,
        function_id: RuntimeFunctionId,
    ) -> HashMap<InstrId, Vec<PersistentFunctionId>> {
        self.for_function(module_name, source_hash, function_id)
            .call_target_specializations
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
    use soac_core::block_py::InstrId;
    use soac_core::profile::{
        CounterDumpKeyLayout, CounterDumpRecord, CounterDumpRow, CounterDumpTypeKey,
        CounterDumpTypeKeyLayout, CounterDumpTypeTableEntry,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn profile_evidence_store_loads_counter_dump_once_into_function_views() {
        let function_id = RuntimeFunctionId::from_raw_parts(7, 1);
        let instr_id = InstrId::new(4);
        let target_id = RuntimeFunctionId::from_raw_parts(7, 2);
        let target_persistent = PersistentFunctionId::new(
            ModuleContentId::new("pkg.mod", 0x1234),
            target_id.local_function_id(),
        );
        let field_specialization = PlannedIndexedFieldSpecialization {
            owner_type: PlannedTypeKey {
                module_name: "pkg.types".to_string(),
                qualname: "Point".to_string(),
            },
            attr_name: "x".to_string(),
            expected_index: 2,
        };
        let global_specialization = PlannedIndexedGlobalSpecialization {
            module_name: "pkg.mod".to_string(),
            name: "G".to_string(),
            expected_index: 1,
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
            module_keys: vec![CounterDumpKeyLayout {
                owner: "pkg.mod".to_string(),
                key: "G".to_string(),
                index: 1,
            }],
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
        assert_eq!(
            store
                .global_index_specializations_for_name("pkg.mod", "G")
                .unwrap(),
            &[global_specialization]
        );
        let v3_evidence = store.evidence_for_runtime_function_v3("pkg.mod", 0x1234, function_id);
        assert_eq!(
            v3_evidence
                .call_target_specializations
                .get(&instr_id)
                .unwrap(),
            &vec![target_id]
        );
        assert_eq!(
            v3_evidence.operator_specializations.get(&instr_id).unwrap(),
            &vec![257]
        );
        assert_eq!(v3_evidence.branch_prefer_true.get(&instr_id), Some(&true));
    }

    #[test]
    fn profile_evidence_store_synthesizes_targets_from_loaded_module_identity() {
        let caller_id = RuntimeFunctionId::from_raw_parts(7, 1);
        let target_id = RuntimeFunctionId::from_raw_parts(8, 2);
        let unrelated_callee_id = RuntimeFunctionId::from_raw_parts(8, 99);
        let instr_id = InstrId::new(4);
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
        assert_eq!(
            store
                .persistent_call_target_specializations_for_runtime_function_v3(
                    "pkg.caller",
                    0x1234,
                    caller_id
                )
                .get(&instr_id),
            Some(&vec![synthesized])
        );
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

    fn unique_counter_path() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac_core-profile-evidence-store-test-{}-{unique}.bin",
            std::process::id()
        ))
    }
}
