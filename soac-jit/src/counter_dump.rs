#[cfg(not(target_endian = "little"))]
compile_error!("counter dump format currently requires little-endian hosts");

use memmap2::Mmap;
use rkyv::rancor::Error as RkyvError;
use rkyv::{Archive, Deserialize, Serialize};
use soac_blockpy::block_py::{BlockLabel, InstrId, RuntimeFunctionId};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub const COUNTER_DUMP_MAGIC: [u8; 8] = *b"SOACRKV1";
pub const COUNTER_DUMP_VERSION: u16 = 3;
const COUNTER_DUMP_FRAME_HEADER_LEN: usize = 32;
const COUNTER_DUMP_FRAME_ALIGN: usize = 16;
pub const COUNTER_DUMP_NONE_U64: u64 = u64::MAX;
pub const COUNTER_DUMP_NONE_FUNCTION_ID: u64 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterDumpRow {
    pub counter_id: u32,
    pub scope: String,
    pub kind: String,
    pub site_kind: String,
    pub function_id: Option<RuntimeFunctionId>,
    pub current_function_id: Option<RuntimeFunctionId>,
    pub instr_id: Option<InstrId>,
    pub function_qualname: Option<String>,
    pub block_label: Option<String>,
    pub value: u64,
    pub observed_value: Option<u64>,
    pub max_overcount: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterDumpRecord {
    pub source_hash: u64,
    pub module_name: String,
    pub package_name: Option<String>,
    pub rows: Vec<CounterDumpRow>,
    pub module_keys: Vec<CounterDumpKeyLayout>,
    pub type_keys: Vec<CounterDumpTypeKeyLayout>,
    pub type_table: Vec<CounterDumpTypeTableEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterDumpKeyLayout {
    pub owner: String,
    pub key: String,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterDumpTypeKeyLayout {
    pub owner_type_id: u64,
    pub key: String,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CounterDumpTypeKey {
    pub module_name: String,
    pub qualname: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterDumpTypeTableEntry {
    pub type_id: u64,
    pub key: CounterDumpTypeKey,
}

pub struct CounterDumpFile {
    mmap: Mmap,
}

#[derive(Clone, Copy)]
pub struct CounterDumpRecordView<'a> {
    record: &'a ArchivedCounterDumpRecordArchive,
}

pub struct CounterDumpRowView<'a> {
    pub counter_id: u32,
    pub scope: &'a str,
    pub kind: &'a str,
    pub site_kind: &'a str,
    pub function_id: Option<RuntimeFunctionId>,
    pub current_function_id: Option<RuntimeFunctionId>,
    pub instr_id: Option<InstrId>,
    pub function_qualname: Option<&'a str>,
    pub block_label: Option<&'a str>,
    pub value: u64,
    pub observed_value: Option<u64>,
    pub max_overcount: Option<u64>,
}

pub struct CounterDumpKeyLayoutView<'a> {
    pub owner: &'a str,
    pub key: &'a str,
    pub index: u32,
}

pub struct CounterDumpTypeKeyLayoutView<'a> {
    pub owner_type_id: u64,
    pub key: &'a str,
    pub index: u32,
}

pub struct CounterDumpTypeTableEntryView<'a> {
    pub type_id: u64,
    pub module_name: &'a str,
    pub qualname: &'a str,
}

#[derive(Archive, Deserialize, Serialize, Debug)]
#[rkyv(derive(Debug))]
struct CounterDumpRecordArchive {
    source_hash: u64,
    module_name: String,
    package_name: String,
    rows: Vec<CounterDumpRowArchive>,
    module_keys: Vec<CounterDumpKeyLayoutArchive>,
    type_keys: Vec<CounterDumpTypeKeyLayoutArchive>,
    type_table: Vec<CounterDumpTypeTableEntryArchive>,
}

#[derive(Archive, Deserialize, Serialize, Debug)]
#[rkyv(derive(Debug))]
struct CounterDumpRowArchive {
    counter_id: u32,
    scope: String,
    kind: String,
    site_kind: String,
    function_id: u64,
    current_function_id: u64,
    instr_block_label: u32,
    instr_index_in_block: u32,
    has_instr_id: bool,
    function_qualname: String,
    has_function_qualname: bool,
    block_label: String,
    has_block_label: bool,
    value: u64,
    observed_value: u64,
    max_overcount: u64,
}

#[derive(Archive, Deserialize, Serialize, Debug)]
#[rkyv(derive(Debug))]
struct CounterDumpKeyLayoutArchive {
    owner: String,
    key: String,
    index: u32,
}

#[derive(Archive, Deserialize, Serialize, Debug)]
#[rkyv(derive(Debug))]
struct CounterDumpTypeKeyLayoutArchive {
    owner_type_id: u64,
    key: String,
    index: u32,
}

#[derive(Archive, Deserialize, Serialize, Debug)]
#[rkyv(derive(Debug))]
struct CounterDumpTypeTableEntryArchive {
    type_id: u64,
    module_name: String,
    qualname: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CallTargetSpecializationEntry {
    module_name: String,
    site_function_id: RuntimeFunctionId,
    instr_id: InstrId,
    observed_function_id: RuntimeFunctionId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectedKeyLayout {
    pub owner: String,
    pub key: String,
    pub index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectedTypeKeyLayout {
    pub owner_type_id: u64,
    pub key: String,
    pub index: u32,
}

impl CounterDumpRecord {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let archive = CounterDumpRecordArchive::from_record(self);
        let payload = rkyv::to_bytes::<RkyvError>(&archive)
            .map_err(|err| format!("failed to rkyv-encode counter dump record: {err}"))?;
        let payload_len = payload.len();
        let frame_len = align_up(
            COUNTER_DUMP_FRAME_HEADER_LEN + payload_len,
            COUNTER_DUMP_FRAME_ALIGN,
        );
        let mut bytes = vec![0u8; frame_len];
        bytes[..8].copy_from_slice(COUNTER_DUMP_MAGIC.as_slice());
        bytes[8..10].copy_from_slice(&COUNTER_DUMP_VERSION.to_le_bytes());
        bytes[10..12].copy_from_slice(&(COUNTER_DUMP_FRAME_HEADER_LEN as u16).to_le_bytes());
        bytes[16..24].copy_from_slice(
            &u64::try_from(payload_len)
                .map_err(|_| "counter dump payload length exceeds u64 capacity".to_string())?
                .to_le_bytes(),
        );
        bytes[COUNTER_DUMP_FRAME_HEADER_LEN..COUNTER_DUMP_FRAME_HEADER_LEN + payload_len]
            .copy_from_slice(payload.as_slice());
        Ok(bytes)
    }
}

impl CounterDumpRecordArchive {
    fn from_record(record: &CounterDumpRecord) -> Self {
        Self {
            source_hash: record.source_hash,
            module_name: record.module_name.clone(),
            package_name: record.package_name.clone().unwrap_or_default(),
            rows: record
                .rows
                .iter()
                .map(CounterDumpRowArchive::from_row)
                .collect(),
            module_keys: record
                .module_keys
                .iter()
                .map(CounterDumpKeyLayoutArchive::from_key_layout)
                .collect(),
            type_keys: record
                .type_keys
                .iter()
                .map(CounterDumpTypeKeyLayoutArchive::from_type_key_layout)
                .collect(),
            type_table: record
                .type_table
                .iter()
                .map(CounterDumpTypeTableEntryArchive::from_entry)
                .collect(),
        }
    }
}

impl CounterDumpRowArchive {
    fn from_row(row: &CounterDumpRow) -> Self {
        let instr_id = row.instr_id;
        Self {
            counter_id: row.counter_id,
            scope: row.scope.clone(),
            kind: row.kind.clone(),
            site_kind: row.site_kind.clone(),
            function_id: row
                .function_id
                .map(RuntimeFunctionId::to_packed_runtime_u64)
                .unwrap_or(COUNTER_DUMP_NONE_FUNCTION_ID),
            current_function_id: row
                .current_function_id
                .map(RuntimeFunctionId::to_packed_runtime_u64)
                .unwrap_or(COUNTER_DUMP_NONE_FUNCTION_ID),
            instr_block_label: instr_id
                .map(|instr_id| instr_id.block_label().as_u32())
                .unwrap_or_default(),
            instr_index_in_block: instr_id
                .map(|instr_id| instr_id.instr_index_in_block())
                .unwrap_or_default(),
            has_instr_id: instr_id.is_some(),
            function_qualname: row.function_qualname.clone().unwrap_or_default(),
            has_function_qualname: row.function_qualname.is_some(),
            block_label: row.block_label.clone().unwrap_or_default(),
            has_block_label: row.block_label.is_some(),
            value: row.value,
            observed_value: row.observed_value.unwrap_or(COUNTER_DUMP_NONE_U64),
            max_overcount: row.max_overcount.unwrap_or(COUNTER_DUMP_NONE_U64),
        }
    }
}

impl CounterDumpKeyLayoutArchive {
    fn from_key_layout(layout: &CounterDumpKeyLayout) -> Self {
        Self {
            owner: layout.owner.clone(),
            key: layout.key.clone(),
            index: layout.index,
        }
    }
}

impl CounterDumpTypeKeyLayoutArchive {
    fn from_type_key_layout(layout: &CounterDumpTypeKeyLayout) -> Self {
        Self {
            owner_type_id: layout.owner_type_id,
            key: layout.key.clone(),
            index: layout.index,
        }
    }
}

impl CounterDumpTypeTableEntryArchive {
    fn from_entry(entry: &CounterDumpTypeTableEntry) -> Self {
        Self {
            type_id: entry.type_id,
            module_name: entry.key.module_name.clone(),
            qualname: entry.key.qualname.clone(),
        }
    }
}

impl CounterDumpFile {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file =
            File::open(path).map_err(|err| format!("failed to open {}: {err}", path.display()))?;
        let mmap = unsafe { Mmap::map(&file) }
            .map_err(|err| format!("failed to map {}: {err}", path.display()))?;
        Ok(Self { mmap })
    }

    pub fn records(&self) -> Result<Vec<CounterDumpRecordView<'_>>, String> {
        parse_counter_dump_records(self.mmap.as_ref())
    }
}

impl<'a> CounterDumpRecordView<'a> {
    pub fn source_hash(&self) -> u64 {
        self.record.source_hash.into()
    }

    pub fn module_name(&self) -> Result<&'a str, String> {
        Ok(self.record.module_name.as_str())
    }

    pub fn package_name(&self) -> Result<Option<&'a str>, String> {
        let package_name = self.record.package_name.as_str();
        if package_name.is_empty() {
            Ok(None)
        } else {
            Ok(Some(package_name))
        }
    }

    pub fn row_count(&self) -> usize {
        self.record.rows.len()
    }

    pub fn row(&self, index: usize) -> Result<CounterDumpRowView<'a>, String> {
        let row = self.record.rows.get(index).ok_or_else(|| {
            format!(
                "counter dump row {index} is out of bounds for {} rows",
                self.row_count()
            )
        })?;
        let function_id: u64 = row.function_id.into();
        let current_function_id: u64 = row.current_function_id.into();
        let instr_block_label: u32 = row.instr_block_label.into();
        let instr_index_in_block: u32 = row.instr_index_in_block.into();
        let observed_value: u64 = row.observed_value.into();
        let max_overcount: u64 = row.max_overcount.into();
        Ok(CounterDumpRowView {
            counter_id: row.counter_id.into(),
            scope: row.scope.as_str(),
            kind: row.kind.as_str(),
            site_kind: row.site_kind.as_str(),
            function_id: (function_id != COUNTER_DUMP_NONE_FUNCTION_ID)
                .then_some(RuntimeFunctionId::from_packed_runtime_u64(function_id)),
            current_function_id: (current_function_id != COUNTER_DUMP_NONE_FUNCTION_ID).then_some(
                RuntimeFunctionId::from_packed_runtime_u64(current_function_id),
            ),
            instr_id: if row.has_instr_id {
                Some(InstrId::new(
                    BlockLabel::from_index(instr_block_label as usize),
                    instr_index_in_block,
                ))
            } else {
                None
            },
            function_qualname: row
                .has_function_qualname
                .then_some(row.function_qualname.as_str()),
            block_label: row.has_block_label.then_some(row.block_label.as_str()),
            value: row.value.into(),
            observed_value: (observed_value != COUNTER_DUMP_NONE_U64).then_some(observed_value),
            max_overcount: (max_overcount != COUNTER_DUMP_NONE_U64).then_some(max_overcount),
        })
    }

    pub fn module_key_count(&self) -> usize {
        self.record.module_keys.len()
    }

    pub fn module_key(&self, index: usize) -> Result<CounterDumpKeyLayoutView<'a>, String> {
        let layout = self.record.module_keys.get(index).ok_or_else(|| {
            format!(
                "counter dump module key {index} is out of bounds for {} keys",
                self.module_key_count()
            )
        })?;
        Ok(CounterDumpKeyLayoutView {
            owner: layout.owner.as_str(),
            key: layout.key.as_str(),
            index: layout.index.into(),
        })
    }

    pub fn type_key_count(&self) -> usize {
        self.record.type_keys.len()
    }

    pub fn type_key(&self, index: usize) -> Result<CounterDumpTypeKeyLayoutView<'a>, String> {
        let layout = self.record.type_keys.get(index).ok_or_else(|| {
            format!(
                "counter dump type key {index} is out of bounds for {} keys",
                self.type_key_count()
            )
        })?;
        Ok(CounterDumpTypeKeyLayoutView {
            owner_type_id: layout.owner_type_id.into(),
            key: layout.key.as_str(),
            index: layout.index.into(),
        })
    }

    pub fn type_table_count(&self) -> usize {
        self.record.type_table.len()
    }

    pub fn type_table_entry(
        &self,
        index: usize,
    ) -> Result<CounterDumpTypeTableEntryView<'a>, String> {
        let entry = self.record.type_table.get(index).ok_or_else(|| {
            format!(
                "counter dump type table entry {index} is out of bounds for {} entries",
                self.type_table_count()
            )
        })?;
        Ok(CounterDumpTypeTableEntryView {
            type_id: entry.type_id.into(),
            module_name: entry.module_name.as_str(),
            qualname: entry.qualname.as_str(),
        })
    }
}

pub fn parse_counter_dump_records(bytes: &[u8]) -> Result<Vec<CounterDumpRecordView<'_>>, String> {
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.len() < COUNTER_DUMP_FRAME_HEADER_LEN {
            return Err(format!(
                "counter dump frame at byte offset {offset} is shorter than the header"
            ));
        }
        if remaining[..8] != COUNTER_DUMP_MAGIC {
            return Err(format!(
                "counter dump frame at byte offset {offset} has invalid magic {:?}",
                &remaining[..8]
            ));
        }
        let version = read_le_u16(remaining, 8)?;
        if version != COUNTER_DUMP_VERSION {
            return Err(format!(
                "counter dump frame at byte offset {offset} uses unsupported version {version}",
            ));
        }
        let header_len = usize::from(read_le_u16(remaining, 10)?);
        if header_len != COUNTER_DUMP_FRAME_HEADER_LEN {
            return Err(format!(
                "counter dump frame at byte offset {offset} has unexpected header size {header_len}"
            ));
        }
        let payload_len = usize::try_from(read_le_u64(remaining, 16)?)
            .map_err(|_| format!("counter dump payload at byte offset {offset} is too large"))?;
        let record_len = align_up(
            COUNTER_DUMP_FRAME_HEADER_LEN + payload_len,
            COUNTER_DUMP_FRAME_ALIGN,
        );
        if record_len == 0 || record_len % COUNTER_DUMP_FRAME_ALIGN != 0 {
            return Err(format!(
                "counter dump frame at byte offset {offset} has invalid length {record_len}"
            ));
        }
        let payload_start = COUNTER_DUMP_FRAME_HEADER_LEN;
        let payload_end = payload_start + payload_len;
        let Some(payload) = remaining.get(payload_start..payload_end) else {
            return Err(format!(
                "counter dump frame at byte offset {offset} extends past end of file"
            ));
        };
        let Some(_record_bytes) = remaining.get(..record_len) else {
            return Err(format!(
                "counter dump frame at byte offset {offset} extends past end of file"
            ));
        };

        let record = rkyv::access::<ArchivedCounterDumpRecordArchive, RkyvError>(payload).map_err(
            |err| {
                format!("failed to rkyv-decode counter dump frame at byte offset {offset}: {err}")
            },
        )?;
        records.push(CounterDumpRecordView { record });
        offset += record_len;
    }
    Ok(records)
}

fn call_target_specialization_entries(
    records: &[CounterDumpRecordView<'_>],
) -> Result<Vec<CallTargetSpecializationEntry>, String> {
    let mut entries = Vec::new();
    for record in records {
        let module_name = record.module_name()?.to_string();
        for row_index in 0..record.row_count() {
            let row = record.row(row_index)?;
            if row.kind != "call_hot_targets" {
                continue;
            }
            let Some(site_function_id) = row.function_id else {
                continue;
            };
            let Some(instr_id) = row.instr_id else {
                continue;
            };
            let Some(observed_value) = row.observed_value else {
                continue;
            };
            if observed_value == 0 {
                continue;
            }
            let observed_function_id = RuntimeFunctionId::from_packed_runtime_u64(observed_value);
            if observed_function_id == RuntimeFunctionId::global() {
                continue;
            }
            entries.push(CallTargetSpecializationEntry {
                module_name: module_name.clone(),
                site_function_id,
                instr_id,
                observed_function_id,
            });
        }
    }
    Ok(entries)
}

fn observed_value_entries_for_kind(
    records: &[CounterDumpRecordView<'_>],
    kind: &str,
) -> Result<Vec<(String, RuntimeFunctionId, InstrId, u64)>, String> {
    let mut entries = Vec::new();
    for record in records {
        let module_name = record.module_name()?.to_string();
        for row_index in 0..record.row_count() {
            let row = record.row(row_index)?;
            if row.kind != kind {
                continue;
            }
            let Some(site_function_id) = row.function_id else {
                continue;
            };
            let Some(instr_id) = row.instr_id else {
                continue;
            };
            let Some(observed_value) = row.observed_value else {
                continue;
            };
            if observed_value == 0 {
                continue;
            }
            entries.push((
                module_name.clone(),
                site_function_id,
                instr_id,
                observed_value,
            ));
        }
    }
    Ok(entries)
}

pub fn render_call_target_specializations(
    records: &[CounterDumpRecordView<'_>],
) -> Result<String, String> {
    let mut ordered_keys = Vec::new();
    let mut seen_targets = HashSet::new();
    let mut targets = HashMap::<String, Vec<u64>>::new();
    for entry in call_target_specialization_entries(records)? {
        let key = format!(
            "{}|{}|{}|{}",
            entry.module_name,
            entry.site_function_id.to_packed_runtime_u64(),
            entry.instr_id.block_label().as_u32(),
            entry.instr_id.instr_index_in_block(),
        );
        let target_key = format!(
            "{key}|{}",
            entry.observed_function_id.to_packed_runtime_u64()
        );
        if seen_targets.insert(target_key) {
            if !targets.contains_key(&key) {
                ordered_keys.push(key.clone());
            }
            targets
                .entry(key)
                .or_default()
                .push(entry.observed_function_id.to_packed_runtime_u64());
        }
    }
    let mut out = String::new();
    for (index, key) in ordered_keys.iter().enumerate() {
        if index > 0 {
            out.push(';');
        }
        out.push_str(key);
        out.push('=');
        let Some(values) = targets.get(key) else {
            return Err(format!("missing specialization targets for key {key}"));
        };
        for (value_index, value) in values.iter().enumerate() {
            if value_index > 0 {
                out.push(',');
            }
            out.push_str(&value.to_string());
        }
    }
    Ok(out)
}

pub fn collect_call_target_specializations_for_function(
    records: &[CounterDumpRecordView<'_>],
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, Vec<RuntimeFunctionId>>, String> {
    let mut out = HashMap::<InstrId, Vec<RuntimeFunctionId>>::new();
    let mut seen_targets = HashSet::<(InstrId, RuntimeFunctionId)>::new();
    for entry in call_target_specialization_entries(records)? {
        if entry.module_name != module_name || entry.site_function_id != function_id {
            continue;
        }
        if seen_targets.insert((entry.instr_id, entry.observed_function_id)) {
            out.entry(entry.instr_id)
                .or_default()
                .push(entry.observed_function_id);
        }
    }
    Ok(out)
}

pub fn read_call_target_specializations_from_file(
    path: &Path,
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, Vec<RuntimeFunctionId>>, String> {
    let dump = CounterDumpFile::open(path)?;
    let records = dump.records()?;
    collect_call_target_specializations_for_function(records.as_slice(), module_name, function_id)
}

pub fn collect_operator_specializations_for_function(
    records: &[CounterDumpRecordView<'_>],
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, Vec<u64>>, String> {
    let mut out = HashMap::<InstrId, Vec<u64>>::new();
    let mut seen_targets = HashSet::<(InstrId, u64)>::new();
    for (entry_module_name, site_function_id, instr_id, observed_value) in
        observed_value_entries_for_kind(records, "operator_hot_shapes")?
    {
        if entry_module_name != module_name || site_function_id != function_id {
            continue;
        }
        if seen_targets.insert((instr_id, observed_value)) {
            out.entry(instr_id).or_default().push(observed_value);
        }
    }
    Ok(out)
}

pub fn collect_getitem_specializations_for_function(
    records: &[CounterDumpRecordView<'_>],
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, Vec<u64>>, String> {
    collect_item_specializations_for_function(
        records,
        module_name,
        function_id,
        "getitem_hot_shapes",
    )
}

pub fn collect_setitem_specializations_for_function(
    records: &[CounterDumpRecordView<'_>],
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, Vec<u64>>, String> {
    collect_item_specializations_for_function(
        records,
        module_name,
        function_id,
        "setitem_hot_shapes",
    )
}

fn collect_item_specializations_for_function(
    records: &[CounterDumpRecordView<'_>],
    module_name: &str,
    function_id: RuntimeFunctionId,
    counter_kind: &str,
) -> Result<HashMap<InstrId, Vec<u64>>, String> {
    let mut out = HashMap::<InstrId, Vec<u64>>::new();
    let mut seen_shapes = HashSet::<(InstrId, u64)>::new();
    for (entry_module_name, site_function_id, instr_id, observed_value) in
        observed_value_entries_for_kind(records, counter_kind)?
    {
        if entry_module_name != module_name || site_function_id != function_id {
            continue;
        }
        if seen_shapes.insert((instr_id, observed_value)) {
            out.entry(instr_id).or_default().push(observed_value);
        }
    }
    Ok(out)
}

pub fn collect_branch_preferences_for_function(
    records: &[CounterDumpRecordView<'_>],
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, bool>, String> {
    let mut counts = HashMap::<InstrId, [u64; 2]>::new();
    for record in records {
        if record.module_name()? != module_name {
            continue;
        }
        for row_index in 0..record.row_count() {
            let row = record.row(row_index)?;
            if row.kind != "branch_outcomes" || row.function_id != Some(function_id) {
                continue;
            }
            let Some(instr_id) = row.instr_id else {
                continue;
            };
            let Some(observed_value) = row.observed_value else {
                continue;
            };
            let Some(slot) = usize::try_from(observed_value)
                .ok()
                .filter(|slot| *slot < 2)
            else {
                continue;
            };
            counts.entry(instr_id).or_default()[slot] =
                counts.entry(instr_id).or_default()[slot].saturating_add(row.value);
        }
    }

    let mut out = HashMap::new();
    for (instr_id, [false_count, true_count]) in counts {
        if false_count == 0 && true_count == 0 {
            continue;
        }
        out.insert(instr_id, true_count >= false_count);
    }
    Ok(out)
}

fn parse_block_label_text(text: &str) -> Option<BlockLabel> {
    let index = text.strip_prefix("bb")?.parse::<usize>().ok()?;
    Some(BlockLabel::from_index(index))
}

pub fn collect_block_entry_counts_for_function(
    records: &[CounterDumpRecordView<'_>],
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<BlockLabel, u64>, String> {
    let mut out = HashMap::<BlockLabel, u64>::new();
    for record in records {
        if record.module_name()? != module_name {
            continue;
        }
        for row_index in 0..record.row_count() {
            let row = record.row(row_index)?;
            if row.kind != "block_entry" || row.function_id != Some(function_id) {
                continue;
            }
            let Some(block_label_text) = row.block_label else {
                continue;
            };
            let Some(block_label) = parse_block_label_text(block_label_text) else {
                continue;
            };
            let count = out.entry(block_label).or_default();
            *count = count.saturating_add(row.value);
        }
    }
    Ok(out)
}

pub fn collect_module_key_layouts(
    records: &[CounterDumpRecordView<'_>],
) -> Result<HashMap<String, Vec<CollectedKeyLayout>>, String> {
    let mut out = HashMap::<String, Vec<CollectedKeyLayout>>::new();
    let mut seen = HashSet::<(String, String, u32)>::new();
    for record in records {
        for key_index in 0..record.module_key_count() {
            let key = record.module_key(key_index)?;
            let seen_key = (key.owner.to_string(), key.key.to_string(), key.index);
            if !seen.insert(seen_key.clone()) {
                continue;
            }
            let (owner, key, index) = seen_key;
            out.entry(owner.clone())
                .or_default()
                .push(CollectedKeyLayout { owner, key, index });
        }
    }
    for layouts in out.values_mut() {
        layouts.sort_by_key(|layout| layout.index);
    }
    Ok(out)
}

pub fn collect_type_key_layouts(
    records: &[CounterDumpRecordView<'_>],
) -> Result<HashMap<u64, Vec<CollectedTypeKeyLayout>>, String> {
    let mut out = HashMap::<u64, Vec<CollectedTypeKeyLayout>>::new();
    let mut seen = HashSet::<(u64, String, u32)>::new();
    for record in records {
        for key_index in 0..record.type_key_count() {
            let key = record.type_key(key_index)?;
            let seen_key = (key.owner_type_id, key.key.to_string(), key.index);
            if !seen.insert(seen_key.clone()) {
                continue;
            }
            let (owner_type_id, key, index) = seen_key;
            out.entry(owner_type_id)
                .or_default()
                .push(CollectedTypeKeyLayout {
                    owner_type_id,
                    key,
                    index,
                });
        }
    }
    for layouts in out.values_mut() {
        layouts.sort_by_key(|layout| layout.index);
    }
    Ok(out)
}

pub fn collect_type_table(
    records: &[CounterDumpRecordView<'_>],
) -> Result<HashMap<u64, CounterDumpTypeKey>, String> {
    let mut out = HashMap::<u64, CounterDumpTypeKey>::new();
    for record in records {
        for index in 0..record.type_table_count() {
            let entry = record.type_table_entry(index)?;
            let key = CounterDumpTypeKey {
                module_name: entry.module_name.to_string(),
                qualname: entry.qualname.to_string(),
            };
            match out.get(&entry.type_id) {
                Some(existing) if existing != &key => {
                    return Err(format!(
                        "counter dump type id {} has conflicting keys {}.{} and {}.{}",
                        entry.type_id,
                        existing.module_name,
                        existing.qualname,
                        key.module_name,
                        key.qualname
                    ));
                }
                Some(_) => {}
                None => {
                    out.insert(entry.type_id, key);
                }
            }
        }
    }
    Ok(out)
}

pub fn read_operator_specializations_from_file(
    path: &Path,
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, Vec<u64>>, String> {
    let dump = CounterDumpFile::open(path)?;
    let records = dump.records()?;
    collect_operator_specializations_for_function(records.as_slice(), module_name, function_id)
}

pub fn read_getitem_specializations_from_file(
    path: &Path,
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, Vec<u64>>, String> {
    let dump = CounterDumpFile::open(path)?;
    let records = dump.records()?;
    collect_getitem_specializations_for_function(records.as_slice(), module_name, function_id)
}

pub fn read_setitem_specializations_from_file(
    path: &Path,
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, Vec<u64>>, String> {
    let dump = CounterDumpFile::open(path)?;
    let records = dump.records()?;
    collect_setitem_specializations_for_function(records.as_slice(), module_name, function_id)
}

pub fn read_branch_preferences_from_file(
    path: &Path,
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<InstrId, bool>, String> {
    let dump = CounterDumpFile::open(path)?;
    let records = dump.records()?;
    collect_branch_preferences_for_function(records.as_slice(), module_name, function_id)
}

pub fn read_block_entry_counts_from_file(
    path: &Path,
    module_name: &str,
    function_id: RuntimeFunctionId,
) -> Result<HashMap<BlockLabel, u64>, String> {
    let dump = CounterDumpFile::open(path)?;
    let records = dump.records()?;
    collect_block_entry_counts_for_function(records.as_slice(), module_name, function_id)
}

fn align_up(offset: usize, align: usize) -> usize {
    let remainder = offset % align;
    if remainder == 0 {
        offset
    } else {
        offset + (align - remainder)
    }
}

fn read_le_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| format!("counter dump u16 at offset {offset} is out of bounds"))?
        .try_into()
        .expect("checked u16 slice should have exact width");
    Ok(u16::from_le_bytes(raw))
}

fn read_le_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| format!("counter dump u64 at offset {offset} is out of bounds"))?
        .try_into()
        .expect("checked u64 slice should have exact width");
    Ok(u64::from_le_bytes(raw))
}

pub fn write_counter_dump_records<'a>(
    path: &Path,
    records: impl IntoIterator<Item = &'a CounterDumpRecord>,
) -> Result<(), String> {
    let mut file =
        File::create(path).map_err(|err| format!("failed to create {}: {err}", path.display()))?;
    for record in records {
        let bytes = record.encode()?;
        file.write_all(bytes.as_slice())
            .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_blockpy::block_py::BlockLabel;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_COUNTER_DUMP_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac_counter_dump_{nonce}_{}_{}.bin",
            std::process::id(),
            NEXT_COUNTER_DUMP_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        let raw = bytes[offset..offset + 2]
            .try_into()
            .expect("u16 slice should have exact width");
        u16::from_le_bytes(raw)
    }

    fn read_u64(bytes: &[u8], offset: usize) -> u64 {
        let raw = bytes[offset..offset + 8]
            .try_into()
            .expect("u64 slice should have exact width");
        u64::from_le_bytes(raw)
    }

    fn key_names_by_owner(
        layouts: HashMap<String, Vec<CollectedKeyLayout>>,
    ) -> HashMap<String, Vec<String>> {
        layouts
            .into_iter()
            .map(|(owner, layouts)| {
                (
                    owner,
                    layouts.into_iter().map(|layout| layout.key).collect(),
                )
            })
            .collect()
    }

    fn type_key_names_by_owner(
        layouts: HashMap<u64, Vec<CollectedTypeKeyLayout>>,
    ) -> HashMap<u64, Vec<String>> {
        layouts
            .into_iter()
            .map(|(owner, layouts)| {
                (
                    owner,
                    layouts.into_iter().map(|layout| layout.key).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn encodes_length_delimited_rkyv_record() {
        let record = CounterDumpRecord {
            source_hash: 0x1234,
            module_name: "counter_test".to_string(),
            package_name: Some("pkg".to_string()),
            module_keys: vec![CounterDumpKeyLayout {
                owner: "counter_test".to_string(),
                key: "module_value".to_string(),
                index: 0,
            }],
            type_keys: vec![CounterDumpTypeKeyLayout {
                owner_type_id: 17,
                key: "x".to_string(),
                index: 1,
            }],
            type_table: vec![CounterDumpTypeTableEntry {
                type_id: 17,
                key: CounterDumpTypeKey {
                    module_name: "counter_test".to_string(),
                    qualname: "Point".to_string(),
                },
            }],
            rows: vec![
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "block_entry".to_string(),
                    site_kind: "block_entry".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: None,
                    function_qualname: Some("f".to_string()),
                    block_label: Some("bb0".to_string()),
                    value: 11,
                    observed_value: None,
                    max_overcount: None,
                },
                CounterDumpRow {
                    counter_id: 4,
                    scope: "global".to_string(),
                    kind: "runtime_incref".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::global()),
                    current_function_id: Some(RuntimeFunctionId::global()),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(5), 3)),
                    function_qualname: None,
                    block_label: None,
                    value: 19,
                    observed_value: Some(42),
                    max_overcount: Some(7),
                },
            ],
        };

        let bytes = record.encode().expect("counter dump should encode");
        assert_eq!(&bytes[..8], COUNTER_DUMP_MAGIC.as_slice());
        assert_eq!(read_u16(&bytes, 8), COUNTER_DUMP_VERSION);
        let header_size = usize::from(read_u16(&bytes, 10));
        let payload_len = read_u64(&bytes, 16) as usize;

        assert_eq!(header_size, COUNTER_DUMP_FRAME_HEADER_LEN);
        assert!(payload_len > 0);
        assert_eq!(bytes.len() % COUNTER_DUMP_FRAME_ALIGN, 0);
        assert!(COUNTER_DUMP_FRAME_HEADER_LEN + payload_len <= bytes.len());

        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        assert_eq!(records.len(), 1);
        let record = records[0];
        assert_eq!(record.source_hash(), 0x1234);
        assert_eq!(record.module_name().expect("module name"), "counter_test");
        assert_eq!(record.package_name().expect("package name"), Some("pkg"));
        assert_eq!(record.row_count(), 2);
        let first_row = record.row(0).expect("first row");
        assert_eq!(first_row.counter_id, 3);
        assert_eq!(first_row.function_id, Some(RuntimeFunctionId::new(1, 7)));
        assert_eq!(first_row.instr_id, None);
        assert_eq!(first_row.block_label, Some("bb0"));
        assert_eq!(first_row.value, 11);
        let second_row = record.row(1).expect("second row");
        assert_eq!(second_row.counter_id, 4);
        assert_eq!(second_row.function_id, Some(RuntimeFunctionId::global()));
        assert_eq!(
            second_row.instr_id,
            Some(InstrId::new(BlockLabel::from_index(5), 3))
        );
        assert_eq!(second_row.observed_value, Some(42));
        assert_eq!(second_row.max_overcount, Some(7));
        let module_key = record.module_key(0).expect("module key");
        assert_eq!(module_key.owner, "counter_test");
        assert_eq!(module_key.key, "module_value");
        assert_eq!(module_key.index, 0);
        let type_key = record.type_key(0).expect("type key");
        assert_eq!(type_key.owner_type_id, 17);
        assert_eq!(type_key.key, "x");
        assert_eq!(type_key.index, 1);
        let type_table_entry = record.type_table_entry(0).expect("type table entry");
        assert_eq!(type_table_entry.type_id, 17);
        assert_eq!(type_table_entry.module_name, "counter_test");
        assert_eq!(type_table_entry.qualname, "Point");
    }

    #[test]
    fn parses_appended_counter_dump_records_from_mmap() {
        let first = CounterDumpRecord {
            source_hash: 0,
            module_name: "alpha".to_string(),
            package_name: Some("pkg".to_string()),
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
            rows: vec![CounterDumpRow {
                counter_id: 1,
                scope: "this".to_string(),
                kind: "block_entry".to_string(),
                site_kind: "block_entry".to_string(),
                function_id: Some(RuntimeFunctionId::new(1, 7)),
                current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                instr_id: None,
                function_qualname: Some("f".to_string()),
                block_label: Some("bb0".to_string()),
                value: 5,
                observed_value: None,
                max_overcount: None,
            }],
        };
        let second = CounterDumpRecord {
            source_hash: 0,
            module_name: "beta".to_string(),
            package_name: None,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
            rows: vec![CounterDumpRow {
                counter_id: 3,
                scope: "global".to_string(),
                kind: "runtime_incref".to_string(),
                site_kind: "runtime".to_string(),
                function_id: Some(RuntimeFunctionId::global()),
                current_function_id: Some(RuntimeFunctionId::global()),
                instr_id: Some(InstrId::new(BlockLabel::from_index(5), 3)),
                function_qualname: None,
                block_label: None,
                value: 11,
                observed_value: Some(7),
                max_overcount: Some(2),
            }],
        };

        let path = temp_path();
        let mut bytes = first.encode().expect("first record should encode");
        bytes.extend_from_slice(
            second
                .encode()
                .expect("second record should encode")
                .as_slice(),
        );
        fs::write(&path, bytes).expect("counter dump file should be writable");

        let dump = CounterDumpFile::open(path.as_path()).expect("counter dump file should map");
        let records = dump.records().expect("mapped counter dump should parse");
        assert_eq!(records.len(), 2);

        let first_record = records[0];
        assert_eq!(first_record.module_name().expect("module name"), "alpha");
        assert_eq!(
            first_record.package_name().expect("package name"),
            Some("pkg")
        );
        let first_row = first_record.row(0).expect("first row should resolve");
        assert_eq!(first_row.counter_id, 1);
        assert_eq!(first_row.scope, "this");
        assert_eq!(first_row.kind, "block_entry");
        assert_eq!(first_row.site_kind, "block_entry");
        assert_eq!(first_row.function_id, Some(RuntimeFunctionId::new(1, 7)));
        assert_eq!(
            first_row.current_function_id,
            Some(RuntimeFunctionId::new(1, 7))
        );
        assert_eq!(first_row.instr_id, None);
        assert_eq!(first_row.function_qualname, Some("f"));
        assert_eq!(first_row.block_label, Some("bb0"));
        assert_eq!(first_row.value, 5);

        let second_record = records[1];
        assert_eq!(second_record.module_name().expect("module name"), "beta");
        assert_eq!(second_record.package_name().expect("package name"), None);
        let second_row = second_record.row(0).expect("second row should resolve");
        assert_eq!(second_row.counter_id, 3);
        assert_eq!(second_row.scope, "global");
        assert_eq!(second_row.kind, "runtime_incref");
        assert_eq!(second_row.site_kind, "runtime");
        assert_eq!(second_row.function_id, Some(RuntimeFunctionId::global()));
        assert_eq!(
            second_row.current_function_id,
            Some(RuntimeFunctionId::global())
        );
        assert_eq!(
            second_row.instr_id,
            Some(InstrId::new(BlockLabel::from_index(5), 3))
        );
        assert_eq!(second_row.function_qualname, None);
        assert_eq!(second_row.block_label, None);
        assert_eq!(second_row.value, 11);

        fs::remove_file(&path).expect("temp counter dump file should be removable");
    }

    #[test]
    fn collects_unique_key_layouts_by_owner() {
        let record = CounterDumpRecord {
            source_hash: 0,
            module_name: "mod".to_string(),
            package_name: None,
            module_keys: vec![
                CounterDumpKeyLayout {
                    owner: "mod".to_string(),
                    key: "b".to_string(),
                    index: 1,
                },
                CounterDumpKeyLayout {
                    owner: "mod".to_string(),
                    key: "a".to_string(),
                    index: 0,
                },
                CounterDumpKeyLayout {
                    owner: "mod".to_string(),
                    key: "a".to_string(),
                    index: 0,
                },
            ],
            type_keys: vec![
                CounterDumpTypeKeyLayout {
                    owner_type_id: 7,
                    key: "y".to_string(),
                    index: 1,
                },
                CounterDumpTypeKeyLayout {
                    owner_type_id: 7,
                    key: "x".to_string(),
                    index: 0,
                },
            ],
            type_table: Vec::new(),
            rows: Vec::new(),
        };
        let bytes = record.encode().expect("counter dump should encode");
        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");

        let module_keys =
            collect_module_key_layouts(&records).expect("module key layouts should collect");
        let type_keys =
            collect_type_key_layouts(&records).expect("type key layouts should collect");

        assert_eq!(
            key_names_by_owner(module_keys),
            HashMap::from([("mod".to_string(), vec!["a".to_string(), "b".to_string()])])
        );
        assert_eq!(
            type_key_names_by_owner(type_keys),
            HashMap::from([(7, vec!["x".to_string(), "y".to_string()])])
        );
    }

    #[test]
    fn collects_type_table_by_id() {
        let record = CounterDumpRecord {
            source_hash: 0,
            module_name: "mod".to_string(),
            package_name: None,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: vec![
                CounterDumpTypeTableEntry {
                    type_id: 10,
                    key: CounterDumpTypeKey {
                        module_name: "mod".to_string(),
                        qualname: "Point".to_string(),
                    },
                },
                CounterDumpTypeTableEntry {
                    type_id: 10,
                    key: CounterDumpTypeKey {
                        module_name: "mod".to_string(),
                        qualname: "Point".to_string(),
                    },
                },
                CounterDumpTypeTableEntry {
                    type_id: 11,
                    key: CounterDumpTypeKey {
                        module_name: "other".to_string(),
                        qualname: "Box".to_string(),
                    },
                },
            ],
            rows: Vec::new(),
        };

        let bytes = record.encode().expect("counter dump should encode");
        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let table = collect_type_table(&records).expect("type table should collect");

        assert_eq!(
            table.get(&10),
            Some(&CounterDumpTypeKey {
                module_name: "mod".to_string(),
                qualname: "Point".to_string(),
            })
        );
        assert_eq!(
            table.get(&11),
            Some(&CounterDumpTypeKey {
                module_name: "other".to_string(),
                qualname: "Box".to_string(),
            })
        );
    }

    #[test]
    fn rejects_conflicting_type_table_ids() {
        let record = CounterDumpRecord {
            source_hash: 0,
            module_name: "mod".to_string(),
            package_name: None,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: vec![
                CounterDumpTypeTableEntry {
                    type_id: 10,
                    key: CounterDumpTypeKey {
                        module_name: "mod".to_string(),
                        qualname: "Point".to_string(),
                    },
                },
                CounterDumpTypeTableEntry {
                    type_id: 10,
                    key: CounterDumpTypeKey {
                        module_name: "other".to_string(),
                        qualname: "Point".to_string(),
                    },
                },
            ],
            rows: Vec::new(),
        };

        let bytes = record.encode().expect("counter dump should encode");
        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let err = collect_type_table(&records).expect_err("conflicting type ids should fail");
        assert!(err.contains("conflicting keys"));
    }

    #[test]
    fn renders_and_collects_call_target_specializations() {
        let record = CounterDumpRecord {
            source_hash: 0,
            module_name: "mod".to_string(),
            package_name: None,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
            rows: vec![
                CounterDumpRow {
                    counter_id: 1,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 11,
                    observed_value: Some(RuntimeFunctionId::new(1, 9).to_packed_runtime_u64()),
                    max_overcount: Some(1),
                },
                CounterDumpRow {
                    counter_id: 2,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 5,
                    observed_value: Some(RuntimeFunctionId::new(1, 10).to_packed_runtime_u64()),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 8)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 1)),
                    function_qualname: Some("pkg.mod.g".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(RuntimeFunctionId::global().to_packed_runtime_u64()),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 4,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 8)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 2)),
                    function_qualname: Some("pkg.mod.h".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(0),
                    max_overcount: Some(0),
                },
            ],
        };

        let bytes = record.encode().expect("counter dump should encode");
        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let rendered =
            render_call_target_specializations(&records).expect("specializations should render");
        assert_eq!(rendered, "mod|4294967303|2|4=4294967305,4294967306");

        let collected = collect_call_target_specializations_for_function(
            &records,
            "mod",
            RuntimeFunctionId::new(1, 7),
        )
        .expect("specializations should collect");
        assert_eq!(
            collected.get(&InstrId::new(BlockLabel::from_index(2), 4)),
            Some(&vec![
                RuntimeFunctionId::new(1, 9),
                RuntimeFunctionId::new(1, 10)
            ])
        );
    }

    #[test]
    fn collect_operator_specializations_filters_and_deduplicates_shapes() {
        let record = CounterDumpRecord {
            source_hash: 0,
            module_name: "mod".to_string(),
            package_name: None,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
            rows: vec![
                CounterDumpRow {
                    counter_id: 1,
                    scope: "this".to_string(),
                    kind: "operator_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 11,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 2,
                    scope: "this".to_string(),
                    kind: "operator_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 5,
                    observed_value: Some(257),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "operator_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 4,
                    scope: "this".to_string(),
                    kind: "operator_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 8)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 1)),
                    function_qualname: Some("pkg.mod.g".to_string()),
                    block_label: None,
                    value: 7,
                    observed_value: Some(513),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 5,
                    scope: "this".to_string(),
                    kind: "operator_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 1,
                    observed_value: Some(0),
                    max_overcount: Some(0),
                },
            ],
        };

        let bytes = record.encode().expect("counter dump should encode");
        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let collected = collect_operator_specializations_for_function(
            &records,
            "mod",
            RuntimeFunctionId::new(1, 7),
        )
        .expect("operator specializations should collect");
        assert_eq!(
            collected.get(&InstrId::new(BlockLabel::from_index(2), 4)),
            Some(&vec![1, 257])
        );
        assert!(
            !collected.contains_key(&InstrId::new(BlockLabel::from_index(3), 1)),
            "operator specializations should filter other functions"
        );
    }

    #[test]
    fn collect_getitem_specializations_filters_and_deduplicates_shapes() {
        let record = CounterDumpRecord {
            source_hash: 0,
            module_name: "mod".to_string(),
            package_name: None,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
            rows: vec![
                CounterDumpRow {
                    counter_id: 1,
                    scope: "this".to_string(),
                    kind: "getitem_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 11,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 2,
                    scope: "this".to_string(),
                    kind: "getitem_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "getitem_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 8)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 1)),
                    function_qualname: Some("pkg.mod.g".to_string()),
                    block_label: None,
                    value: 7,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 4,
                    scope: "this".to_string(),
                    kind: "getitem_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 1,
                    observed_value: Some(0),
                    max_overcount: Some(0),
                },
            ],
        };

        let bytes = record.encode().expect("counter dump should encode");
        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let collected = collect_getitem_specializations_for_function(
            &records,
            "mod",
            RuntimeFunctionId::new(1, 7),
        )
        .expect("getitem specializations should collect");
        assert_eq!(
            collected.get(&InstrId::new(BlockLabel::from_index(2), 4)),
            Some(&vec![1])
        );
        assert!(
            !collected.contains_key(&InstrId::new(BlockLabel::from_index(3), 1)),
            "getitem specializations should filter other functions"
        );
    }

    #[test]
    fn collect_setitem_specializations_filters_and_deduplicates_shapes() {
        let record = CounterDumpRecord {
            source_hash: 0,
            module_name: "mod".to_string(),
            package_name: None,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
            rows: vec![
                CounterDumpRow {
                    counter_id: 1,
                    scope: "this".to_string(),
                    kind: "setitem_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 11,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 2,
                    scope: "this".to_string(),
                    kind: "setitem_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "setitem_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 8)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 1)),
                    function_qualname: Some("pkg.mod.g".to_string()),
                    block_label: None,
                    value: 7,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 4,
                    scope: "this".to_string(),
                    kind: "setitem_hot_shapes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 1,
                    observed_value: Some(0),
                    max_overcount: Some(0),
                },
            ],
        };

        let bytes = record.encode().expect("counter dump should encode");
        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let collected = collect_setitem_specializations_for_function(
            &records,
            "mod",
            RuntimeFunctionId::new(1, 7),
        )
        .expect("setitem specializations should collect");
        assert_eq!(
            collected.get(&InstrId::new(BlockLabel::from_index(2), 4)),
            Some(&vec![1])
        );
        assert!(
            !collected.contains_key(&InstrId::new(BlockLabel::from_index(3), 1)),
            "setitem specializations should filter other functions"
        );
    }

    #[test]
    fn collect_branch_preferences_compares_false_and_true_counts() {
        let hot_false_site = InstrId::new(BlockLabel::from_index(2), 4);
        let hot_true_site = InstrId::new(BlockLabel::from_index(3), 5);
        let record = CounterDumpRecord {
            source_hash: 0,
            module_name: "mod".to_string(),
            package_name: None,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
            rows: vec![
                CounterDumpRow {
                    counter_id: 1,
                    scope: "this".to_string(),
                    kind: "branch_outcomes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(hot_false_site),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 20,
                    observed_value: Some(0),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 2,
                    scope: "this".to_string(),
                    kind: "branch_outcomes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(hot_false_site),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 3,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "branch_outcomes".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::new(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::new(1, 7)),
                    instr_id: Some(hot_true_site),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 9,
                    observed_value: Some(1),
                    max_overcount: Some(0),
                },
            ],
        };

        let bytes = record.encode().expect("counter dump should encode");
        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let collected =
            collect_branch_preferences_for_function(&records, "mod", RuntimeFunctionId::new(1, 7))
                .expect("branch preferences should collect");

        assert_eq!(collected.get(&hot_false_site), Some(&false));
        assert_eq!(collected.get(&hot_true_site), Some(&true));
    }
}
